import Foundation
#if canImport(TallyCamCoreFFI)
    import TallyCamCoreFFI
#endif

/// Auto ISO: the core's loop (lc_auto_iso_*) that sets the ISO from the
/// metered exposure and keeps the shutter flicker-free. Feed it a sample a
/// few times a second on one queue; apply what it returns.
final class AutoIsoLoop {
    enum Limit {
        case none
        /// At the highest ISO and still too dark.
        case tooDark
        /// At the lowest ISO and still too bright.
        case tooBright
    }

    struct Input {
        var tNs: Int64
        /// exposureTargetOffset, EV.
        var offsetEv: Double
        var iso: Double
        var exposureNs: Double
        var minIso: Double
        var maxIso: Double
        var minExposureNs: Double
        /// From `longestExposureNs`.
        var maxExposureNs: Double
    }

    struct Command {
        var iso: Double
        var exposureNs: Double
    }

    struct Status {
        var errorEv: Double
        var gain: Double
        var limit: Limit
        var settling: Bool
    }

    private let loop: OpaquePointer

    /// `mainsHz` 50 or 60 rounds the shutter flicker-free; 0 leaves it.
    init(mainsHz: Int) {
        loop = lc_auto_iso_new(UInt32(max(mainsHz, 0)))
    }

    deinit {
        lc_auto_iso_free(loop)
    }

    /// Starts over; keeps the learned metering gain and the bias.
    func reset() {
        lc_auto_iso_reset(loop)
    }

    func setBias(ev: Double) {
        lc_auto_iso_set_bias(loop, ev)
    }

    func setMains(hz: Int) {
        lc_auto_iso_set_mains(loop, UInt32(max(hz, 0)))
    }

    /// The ISO and exposure duration to apply, when they change.
    func onSample(_ input: Input) -> Command? {
        var i = LcAutoIsoInput(t_ns: input.tNs, offset_ev: input.offsetEv, iso: input.iso,
                               exposure_ns: input.exposureNs, min_iso: input.minIso, max_iso: input.maxIso,
                               min_exposure_ns: input.minExposureNs, max_exposure_ns: input.maxExposureNs)
        var iso = 0.0
        var exposure = 0.0
        return lc_auto_iso_on_sample(loop, &i, &iso, &exposure) ? Command(iso: iso, exposureNs: exposure) : nil
    }

    var status: Status {
        var s = LcAutoIsoStatus()
        lc_auto_iso_status(loop, &s)
        let limit: Limit = switch Int32(s.limit) {
        case LC_AUTO_ISO_LIMIT_TOO_DARK: .tooDark
        case LC_AUTO_ISO_LIMIT_TOO_BRIGHT: .tooBright
        default: .none
        }
        return Status(errorEv: s.error_ev, gain: s.gain, limit: limit, settling: s.settling)
    }

    /// The longest exposure for a frame period: the format's maximum, the
    /// frame, and room for genlock's shorter frames while it runs.
    static func longestExposureNs(framePeriodNs: Double, formatMaxNs: Double, genlock: Bool) -> Double {
        lc_longest_exposure_ns(framePeriodNs, formatMaxNs, genlock)
    }
}
