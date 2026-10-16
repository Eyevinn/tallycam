import Foundation
#if canImport(TallyCamCoreFFI)
    import TallyCamCoreFFI
#endif

/// Soft genlock: the core's PI loop (lc_genlock_*) that locks the camera's
/// frame timing to the TAI frame grid. Feed it every captured frame on one
/// queue; it returns a frame duration to ask the device for about once a
/// second.
final class GenlockLoop {
    enum State: String {
        case freeRun = "free_run"
        case acquiring
        case locked
    }

    struct Status {
        var state: State
        var phaseErrorMs: Double
        var phaseErrorMaxMs: Double
        var frameDurationNs: Double
        var correctionPpm: Double
    }

    let fps: Int
    private let loop: OpaquePointer

    /// `minFrameDurationNs`: the active format's shortest frame.
    init?(fps: Int, minFrameDurationNs: Double) {
        guard fps > 0, let loop = lc_genlock_new(UInt32(fps), 1, minFrameDurationNs) else {
            return nil
        }
        self.fps = fps
        self.loop = loop
    }

    deinit {
        lc_genlock_free(loop)
    }

    /// Returns the frame duration (ns) to apply, when the loop has a new one.
    func onFrame(captureTaiNs: Int64, synced: Bool) -> Double? {
        var duration = 0.0
        return lc_genlock_on_frame(loop, captureTaiNs, synced, &duration) ? duration : nil
    }

    /// Frames land this far after each grid point. Same queue as `onFrame`.
    func setTargetOffset(ns: Int64) {
        lc_genlock_set_target_offset(loop, ns)
    }

    /// Same queue as `onFrame`.
    var status: Status {
        var s = LcGenlockStatus()
        lc_genlock_status(loop, &s)
        let state: State = switch Int32(s.state) {
        case LC_GENLOCK_LOCKED: .locked
        case LC_GENLOCK_ACQUIRING: .acquiring
        default: .freeRun
        }
        return Status(state: state, phaseErrorMs: s.phase_error_ns / 1e6, phaseErrorMaxMs: s.phase_error_max_ns / 1e6,
                      frameDurationNs: s.frame_duration_ns, correctionPpm: s.correction_ppm)
    }
}
