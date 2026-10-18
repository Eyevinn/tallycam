import Accelerate
import Atomics
@preconcurrency import CoreMedia
import CoreGraphics
import Foundation

/// Focus peaking as an overlay (docs/SHADING.md, D10): the sharp edges of
/// the streamed frame as a mask over the preview, at a low rate. Only runs
/// while peaking is on (`start` installs the capture tap, `stop` removes it).
@MainActor
final class FocusPeaking: ObservableObject {
    /// White where the frame has a sharp edge, transparent elsewhere, in the
    /// streamed frame's orientation (tinted by the view).
    @Published private(set) var mask: CGImage?

    private var processor: PeakingProcessor?
    private var outputs: CaptureOutputs?
    /// Bumped on stop, so a mask still in flight is not shown afterwards.
    private var generation = 0

    func start(outputs: CaptureOutputs) {
        guard processor == nil else {
            return
        }
        let generation = generation
        let processor = PeakingProcessor { [weak self] mask in
            Task { @MainActor in
                guard let self, self.generation == generation else {
                    return
                }
                self.mask = mask.image
            }
        }
        self.processor = processor
        self.outputs = outputs
        outputs.setPeakingSink { processor.offer($0) }
    }

    func stop() {
        outputs?.setPeakingSink(nil)
        outputs = nil
        processor = nil
        generation += 1
        mask = nil
    }
}

/// The work behind `FocusPeaking`. `offer` runs on the capture video queue
/// and only checks two atomics and, at most `fps` times a second, retains
/// one buffer; the edge detection runs on a utility queue. A frame is
/// dropped while the previous one is in flight.
final class PeakingProcessor: @unchecked Sendable {
    struct Mask: @unchecked Sendable {
        let image: CGImage?
    }

    private static let fps: UInt64 = 9
    /// Width of the downscaled luma plane the edges are found in.
    private static let width = 320
    /// Share of the picture the strongest edges may cover...
    private static let edgeShare = 0.07
    /// ...but never below this gradient (|gx| + |gy| of a 3x3 Sobel, 8-bit
    /// luma): a soft picture shows no edges rather than its noise.
    private static let minimumGradient = 150

    private let onMask: @Sendable (Mask) -> Void
    /// User initiated: the frame it holds belongs to the capture pool, and
    /// utility work is the first to be throttled when the phone is hot.
    private let queue = DispatchQueue(label: "peaking", qos: .userInitiated)
    private static let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)
    private let inFlight = ManagedAtomic<Bool>(false)
    private let lastNs = ManagedAtomic<UInt64>(0)
    // Owned by `queue`.
    private var small: [UInt8] = []
    private var gradient: [UInt16] = []
    private var histogram = [Int](repeating: 0, count: 256)

    init(onMask: @escaping @Sendable (Mask) -> Void) {
        self.onMask = onMask
    }

    func offer(_ sampleBuffer: CMSampleBuffer) {
        let now = DispatchTime.now().uptimeNanoseconds
        guard now &- lastNs.load(ordering: .relaxed) >= 1_000_000_000 / Self.fps,
              inFlight.compareExchange(expected: false, desired: true, ordering: .acquiring).exchanged
        else {
            return
        }
        lastNs.store(now, ordering: .relaxed)
        // CVPixelBuffer is safe to hand to another queue once captured.
        let frame = PeakingFrame(sampleBuffer: sampleBuffer)
        queue.async { [self] in
            if let pixelBuffer = frame.sampleBuffer.imageBuffer {
                onMask(Mask(image: edges(pixelBuffer)))
            }
            inFlight.store(false, ordering: .releasing)
        }
    }

    /// Downscales the luma plane, runs a Sobel filter and keeps the
    /// strongest edges.
    private func edges(_ pixelBuffer: CVPixelBuffer) -> CGImage? {
        let format = CVPixelBufferGetPixelFormatType(pixelBuffer)
        guard format == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
            || format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
            CVPixelBufferGetPlaneCount(pixelBuffer) == 2
        else {
            return nil
        }
        guard let (width, height) = downscale(pixelBuffer), width >= 3, height >= 3 else {
            return nil
        }
        if gradient.count != width * height {
            gradient = [UInt16](repeating: 0, count: width * height)
        }
        for i in histogram.indices {
            histogram[i] = 0
        }
        // Raw buffers: no exclusivity, bounds or copy-on-write checks per
        // pixel on `self`'s arrays.
        small.withUnsafeBufferPointer { luma in
            gradient.withUnsafeMutableBufferPointer { out in
              histogram.withUnsafeMutableBufferPointer { histogram in
                for y in 1 ..< height - 1 {
                    let above = (y - 1) * width, row = y * width, below = (y + 1) * width
                    for x in 1 ..< width - 1 {
                        let a = Int(luma[above + x - 1]), b = Int(luma[above + x]), c = Int(luma[above + x + 1])
                        let d = Int(luma[row + x - 1]), f = Int(luma[row + x + 1])
                        let g = Int(luma[below + x - 1]), h = Int(luma[below + x]), i = Int(luma[below + x + 1])
                        let gx = (c + 2 * f + i) - (a + 2 * d + g)
                        let gy = (g + 2 * h + i) - (a + 2 * b + c)
                        let magnitude = abs(gx) + abs(gy)
                        out[row + x] = UInt16(magnitude)
                        histogram[min(magnitude >> 3, 255)] += 1
                    }
                }
              }
            }
        }
        let threshold = max(Self.minimumGradient, Self.percentile(histogram, share: Self.edgeShare,
                                                                  total: (width - 2) * (height - 2)))
        var rgba = Data(count: width * height * 4)
        rgba.withUnsafeMutableBytes { raw in
            let pixels = raw.bindMemory(to: UInt32.self)
            gradient.withUnsafeBufferPointer { gradient in
                for i in 0 ..< width * height where Int(gradient[i]) >= threshold {
                    // Opaque white, premultiplied.
                    pixels[i] = 0xFFFF_FFFF
                }
            }
        }
        guard let provider = CGDataProvider(data: rgba as CFData), let colorSpace = Self.colorSpace else {
            return nil
        }
        return CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
                       bytesPerRow: width * 4, space: colorSpace,
                       bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                       provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }

    /// The luma plane scaled to `width` pixels wide into `small`.
    private func downscale(_ pixelBuffer: CVPixelBuffer) -> (Int, Int)? {
        CVPixelBufferLockBaseAddress(pixelBuffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, .readOnly) }
        let sourceWidth = CVPixelBufferGetWidthOfPlane(pixelBuffer, 0)
        let sourceHeight = CVPixelBufferGetHeightOfPlane(pixelBuffer, 0)
        guard sourceWidth > 0, sourceHeight > 0,
              let base = CVPixelBufferGetBaseAddressOfPlane(pixelBuffer, 0)
        else {
            return nil
        }
        let width = min(Self.width, sourceWidth)
        let height = max(1, Int((Double(sourceHeight) * Double(width) / Double(sourceWidth)).rounded()))
        if small.count != width * height {
            small = [UInt8](repeating: 0, count: width * height)
        }
        var source = vImage_Buffer(data: base, height: vImagePixelCount(sourceHeight),
                                   width: vImagePixelCount(sourceWidth),
                                   rowBytes: CVPixelBufferGetBytesPerRowOfPlane(pixelBuffer, 0))
        let error = small.withUnsafeMutableBytes { out in
            var destination = vImage_Buffer(data: out.baseAddress, height: vImagePixelCount(height),
                                            width: vImagePixelCount(width), rowBytes: width)
            return vImageScale_Planar8(&source, &destination, nil, vImage_Flags(kvImageNoFlags))
        }
        return error == kvImageNoError ? (width, height) : nil
    }

    /// The gradient above which `share` of the pixels lie (histogram bins
    /// of 8).
    private static func percentile(_ histogram: [Int], share: Double, total: Int) -> Int {
        let wanted = Int(Double(total) * share)
        var count = 0
        for bin in histogram.indices.reversed() {
            count += histogram[bin]
            if count >= wanted {
                return bin << 3
            }
        }
        return 0
    }
}

private struct PeakingFrame: @unchecked Sendable {
    let sampleBuffer: CMSampleBuffer
}
