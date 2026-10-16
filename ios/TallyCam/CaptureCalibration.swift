import Foundation

/// Per-model correction of the capture timestamp. Models stamp the same
/// exposure at different points (where in the readout the stamp sits, how
/// fast the rolling shutter is), so the same stamp is not the same instant of
/// light on every model. The lead is how much earlier than the reference
/// (iPhone 16) a model exposes for the same stamp; subtracting it from the
/// stamp makes video timestamps comparable across models, for genlock, the
/// SEI and the PTS alike.
///
/// To be measured with a stable light (an LED on the TAI second) placed at the
/// same height in both frames, since rows of a rolling shutter expose at
/// different times (docs/GENLOCK.md, "Two cameras").
enum CaptureCalibration {
    /// Exposure lead per hardware model (`utsname.machine`), ns, at 1080p30.
    static let leadNs: [String: Int64] = [
        // iPhone 16: the reference.
        "iPhone17,3": 0,
        // iPad (10th generation): a display flash suggested about 7 ms, but
        // with that value the two cameras did not agree better, so it is
        // not used until a stable light measures it (docs/GENLOCK.md).
        "iPad13,18": 0,
        "iPad13,19": 0,
    ]

    static let model: String = {
        var info = utsname()
        uname(&info)
        return withUnsafeBytes(of: &info.machine) { buffer in
            String(decoding: buffer.prefix { $0 != 0 }, as: UTF8.self)
        }
    }()

    /// This device's lead, ns; 0 for models not measured yet.
    static let exposureLeadNs: Int64 = leadNs[model] ?? 0

    /// The capture stamp corrected to the reference model's timescale.
    static func exposureNs(stampNs: Int64) -> Int64 {
        stampNs - exposureLeadNs
    }
}
