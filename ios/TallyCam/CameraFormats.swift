import Foundation
#if canImport(TallyCamCoreFFI)
    import TallyCamCoreFFI
#endif

/// The stream resolutions offered: 16:9, landscape sizes. The control
/// `resolution` lists the ones the active lens has.
enum VideoResolution: String, CaseIterable, Comparable {
    case p720 = "720p"
    case p1080 = "1080p"
    case p2160 = "2160p"

    static let defaultResolution = VideoResolution.p1080

    var width: Int32 {
        switch self {
        case .p720: 1280
        case .p1080: 1920
        case .p2160: 3840
        }
    }

    var height: Int32 {
        switch self {
        case .p720: 720
        case .p1080: 1080
        case .p2160: 2160
        }
    }

    var label: String {
        switch self {
        case .p720: "720p"
        case .p1080: "1080p"
        case .p2160: "2160p (4K)"
        }
    }

    init?(width: Int32, height: Int32) {
        guard let resolution = Self.allCases.first(where: { $0.width == width && $0.height == height }) else {
            return nil
        }
        self = resolution
    }

    static func < (lhs: VideoResolution, rhs: VideoResolution) -> Bool {
        lhs.height < rhs.height
    }
}

/// The active capture format: landscape size (as the sensor delivers it)
/// and frame rate.
struct CaptureFormat: Equatable, Sendable {
    var width: Int32
    var height: Int32
    var frameRate: Int

    static let initial = CaptureFormat(width: VideoResolution.defaultResolution.width,
                                       height: VideoResolution.defaultResolution.height, frameRate: 30)

    var landscapeSize: (width: Int32, height: Int32) {
        (width, height)
    }
}

/// What format selection needs from an `AVCaptureDevice.Format`, so the
/// choice can be tested without a camera.
struct CameraFormatInfo: Equatable {
    struct RateRange: Equatable {
        var min: Double
        var max: Double
    }

    var width: Int32
    var height: Int32
    /// 8-bit 4:2:0 bi-planar (420v or 420f): SDR. 10-bit formats (HDR,
    /// Dolby Vision) are not used.
    var is8Bit420: Bool
    /// 420v: what the video output delivers without conversion.
    var isVideoRange: Bool
    var isBinned: Bool
    var rateRanges: [RateRange]

    var resolution: VideoResolution? {
        is8Bit420 ? VideoResolution(width: width, height: height) : nil
    }

    var maxFrameRate: Double {
        rateRanges.map(\.max).max() ?? 0
    }

    /// Whether a range holds the rate exactly (AVFoundation refuses frame
    /// durations outside the range, so 30 is not in 29.97); the margin is
    /// only for floating point.
    func supports(frameRate: Int) -> Bool {
        let rate = Double(frameRate)
        return rateRanges.contains { rate >= $0.min - 0.001 && rate <= $0.max + 0.001 }
    }
}

/// Picks capture formats from what the active lens reports.
enum CameraFormats {
    /// The frame rates offered, where a format has them.
    static let frameRates = [24, 25, 30, 50, 60, 100, 120, 240]

    struct FrameRateOption: Equatable {
        var frameRate: Int
        /// The best format for it is sensor binned (lower detail, the
        /// usual price of high frame rates).
        var binned: Bool
    }

    /// The resolutions the formats have, smallest first.
    static func resolutions(_ formats: [CameraFormatInfo]) -> [VideoResolution] {
        Set(formats.compactMap(\.resolution)).sorted()
    }

    /// The frame rates offered at `resolution`, with whether the format
    /// used for each is binned.
    static func frameRates(_ formats: [CameraFormatInfo], resolution: VideoResolution) -> [FrameRateOption] {
        frameRates.compactMap { rate in
            best(formats, resolution: resolution, frameRate: rate)
                .map { FrameRateOption(frameRate: rate, binned: formats[$0].isBinned) }
        }
    }

    /// Index of the format for `resolution` at `frameRate`: unbinned first,
    /// then the one with the lowest maximum rate (high-speed formats have
    /// more restrictions), then 420v (no conversion in the video output).
    /// With `headroom` (genlock), a format that also runs a little faster
    /// than `frameRate` comes before the lowest maximum rate: the loop needs
    /// frames slightly shorter than nominal to catch up.
    static func best(_ formats: [CameraFormatInfo], resolution: VideoResolution, frameRate: Int,
                     headroom: Bool = false) -> Int?
    {
        let roomy = { (f: CameraFormatInfo) in f.maxFrameRate > Double(frameRate) * 1.01 }
        return formats.indices
            .filter { formats[$0].resolution == resolution && formats[$0].supports(frameRate: frameRate) }
            .min { lhs, rhs in
                let l = formats[lhs], r = formats[rhs]
                if l.isBinned != r.isBinned {
                    return !l.isBinned
                }
                if headroom, roomy(l) != roomy(r) {
                    return roomy(l)
                }
                if l.maxFrameRate != r.maxFrameRate {
                    return l.maxFrameRate < r.maxFrameRate
                }
                return l.isVideoRange && !r.isVideoRange
            }
    }

    /// `wanted` if the lens has it, else the largest it has below it, else
    /// the smallest above it.
    static func fallback(_ wanted: VideoResolution, available: [VideoResolution]) -> VideoResolution? {
        if available.contains(wanted) {
            return wanted
        }
        return available.filter { $0 < wanted }.max() ?? available.min()
    }

    /// `wanted` if offered, else the nearest lower rate, else the lowest.
    static func fallback(frameRate wanted: Int, available: [Int]) -> Int? {
        if available.contains(wanted) {
            return wanted
        }
        return available.filter { $0 < wanted }.max() ?? available.min()
    }
}

/// Default and maximum video bitrate for a format and codec, from the core
/// (core/src/bitrate.rs): 6 Mbit/s for 1080p30 H.264, scaled with the pixel
/// rate, about 30% less for HEVC.
enum VideoBitrate {
    static let minKbps = Int(videoBitrateMinKbps())

    static func defaultKbps(width: Int32, height: Int32, frameRate: Int, codec: VideoCodec) -> Int {
        Int(videoBitrateDefaultKbps(width: UInt32(max(width, 0)), height: UInt32(max(height, 0)),
                                    frameRate: UInt32(max(frameRate, 0)), codec: codec.bitrateCodec))
    }

    /// The top of the `bitrate_kbps` range.
    static func maxKbps(width: Int32, height: Int32, frameRate: Int, codec: VideoCodec) -> Int {
        Int(videoBitrateMaxKbps(width: UInt32(max(width, 0)), height: UInt32(max(height, 0)),
                                frameRate: UInt32(max(frameRate, 0)), codec: codec.bitrateCodec))
    }
}

private extension VideoCodec {
    var bitrateCodec: BitrateCodec {
        switch self {
        case .h264: .h264
        case .hevc: .hevc
        }
    }
}
