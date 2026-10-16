@preconcurrency import CoreImage
@preconcurrency import CoreMedia
import Foundation
import ImageIO

/// Low-rate JPEG snapshots of the video frames for the operator UI.
/// `offer` runs on the capture video queue and only takes a lock and, at
/// most `fps` times a second, retains one pixel buffer; scaling and JPEG
/// encoding happen on a background queue. A frame is dropped while the
/// previous one is still being encoded.
final class PreviewEncoder: @unchecked Sendable {
    /// Called on the preview queue with JPEG data and its size.
    var onJpeg: (@Sendable (Data, Int, Int) -> Void)?

    private let queue = DispatchQueue(label: "preview.encode", qos: .utility)
    private let context = CIContext(options: [.cacheIntermediates: false])
    private let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)
    private let lock = NSLock()
    // Guarded by `lock`.
    private var interval: TimeInterval = 1
    private var width = 480
    private var inFlight = false
    private var lastTime: TimeInterval = 0

    func configure(fps: Double, width: Int) {
        lock.withLock {
            interval = 1 / min(max(fps, 0.1), 10)
            self.width = min(max(width, 64), 1920) & ~1
            lastTime = 0
        }
    }

    func offer(_ sampleBuffer: CMSampleBuffer) {
        let now = ProcessInfo.processInfo.systemUptime
        let width: Int? = lock.withLock {
            guard !inFlight, now - lastTime >= interval else {
                return nil
            }
            inFlight = true
            lastTime = now
            return self.width
        }
        guard let width else {
            return
        }
        // CVPixelBuffer is safe to hand to another queue once captured.
        let frame = Frame(sampleBuffer: sampleBuffer)
        queue.async { [self] in
            if let pixelBuffer = frame.sampleBuffer.imageBuffer {
                encode(pixelBuffer, width: width)
            }
            lock.withLock { inFlight = false }
        }
    }

    private func encode(_ pixelBuffer: CVPixelBuffer, width: Int) {
        let image = CIImage(cvPixelBuffer: pixelBuffer)
        let extent = image.extent
        guard extent.width > 0, let colorSpace else {
            return
        }
        let scale = min(CGFloat(width) / extent.width, 1)
        let scaled = image.transformed(by: CGAffineTransform(scaleX: scale, y: scale))
        let outWidth = Int((extent.width * scale).rounded())
        let outHeight = Int((extent.height * scale).rounded())
        let options = [CIImageRepresentationOption(rawValue: kCGImageDestinationLossyCompressionQuality as String): 0.5]
        guard let jpeg = context.jpegRepresentation(of: scaled, colorSpace: colorSpace, options: options) else {
            return
        }
        onJpeg?(jpeg, outWidth, outHeight)
    }
}

private struct Frame: @unchecked Sendable {
    let sampleBuffer: CMSampleBuffer
}
