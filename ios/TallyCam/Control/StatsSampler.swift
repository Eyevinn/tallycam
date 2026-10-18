@preconcurrency import CoreMedia
import CoreVideo
import Foundation
#if canImport(TallyCamCoreFFI)
    import TallyCamCoreFFI
#endif

/// Image statistics (docs/SHADING.md) from the frames that are encoded: a
/// coarse grid of luma and chroma samples, handed to the core a few times a
/// second. `offer` runs on the capture video queue and only takes a lock and,
/// at most `fps` times a second, retains one pixel buffer; the sampling runs
/// on a utility queue. A frame is skipped while the previous one is still
/// being measured.
final class StatsSampler: @unchecked Sendable {
    /// Columns of the grid; rows follow the aspect ratio.
    static let gridWidth = 160

    /// Called on the sampler queue.
    var onStats: (@Sendable (ImageStatsResult) -> Void)?

    /// User initiated: it holds a capture-pool frame while it samples.
    private let queue = DispatchQueue(label: "stats.sample", qos: .userInitiated)
    private let lock = NSLock()
    // Guarded by `lock`.
    private var interval: TimeInterval = 0.5
    private var inFlight = false
    private var lastTime: TimeInterval = 0

    /// Measurements per second, 0.2 to 4.
    func setFps(_ fps: Double) {
        lock.withLock {
            interval = 1 / min(max(fps, 0.2), 4)
        }
    }

    func offer(_ sampleBuffer: CMSampleBuffer) {
        let now = ProcessInfo.processInfo.systemUptime
        let take = lock.withLock {
            guard !inFlight, now - lastTime >= interval else {
                return false
            }
            inFlight = true
            lastTime = now
            return true
        }
        guard take else {
            return
        }
        let frame = StatsFrame(sampleBuffer: sampleBuffer)
        queue.async { [self] in
            if let pixelBuffer = frame.sampleBuffer.imageBuffer, let stats = Self.measure(pixelBuffer) {
                onStats?(stats)
            }
            lock.withLock { inFlight = false }
        }
    }

    /// Samples every `step`-th pixel of an 8-bit 4:2:0 bi-planar buffer.
    static func measure(_ pixelBuffer: CVPixelBuffer) -> ImageStatsResult? {
        let format = CVPixelBufferGetPixelFormatType(pixelBuffer)
        guard format == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
            || format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
            CVPixelBufferGetPlaneCount(pixelBuffer) == 2
        else {
            return nil
        }
        CVPixelBufferLockBaseAddress(pixelBuffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, .readOnly) }
        let width = CVPixelBufferGetWidthOfPlane(pixelBuffer, 0)
        let height = CVPixelBufferGetHeightOfPlane(pixelBuffer, 0)
        guard width >= 2, height >= 2,
              let lumaBase = CVPixelBufferGetBaseAddressOfPlane(pixelBuffer, 0),
              let chromaBase = CVPixelBufferGetBaseAddressOfPlane(pixelBuffer, 1)
        else {
            return nil
        }
        // Even, so every sample sits on a chroma site.
        let step = max(2, (width / gridWidth) & ~1)
        let columns = width / step
        let rows = height / step
        let lumaStride = CVPixelBufferGetBytesPerRowOfPlane(pixelBuffer, 0)
        let chromaStride = CVPixelBufferGetBytesPerRowOfPlane(pixelBuffer, 1)
        let luma = lumaBase.assumingMemoryBound(to: UInt8.self)
        let chroma = chromaBase.assumingMemoryBound(to: UInt8.self)
        var y = Data(count: columns * rows)
        var cbcr = Data(count: 2 * columns * rows)
        y.withUnsafeMutableBytes { yOut in
            cbcr.withUnsafeMutableBytes { cOut in
                var i = 0
                for row in 0 ..< rows {
                    let lumaRow = luma + row * step * lumaStride
                    let chromaRow = chroma + (row * step / 2) * chromaStride
                    for column in 0 ..< columns {
                        let x = column * step
                        yOut[i] = lumaRow[x]
                        cOut[2 * i] = chromaRow[x]
                        cOut[2 * i + 1] = chromaRow[x + 1]
                        i += 1
                    }
                }
            }
        }
        return imageStatsCompute(y: y, cbcr: cbcr, width: UInt32(columns), height: UInt32(rows),
                                 videoRange: format == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange)
    }
}

private struct StatsFrame: @unchecked Sendable {
    let sampleBuffer: CMSampleBuffer
}
