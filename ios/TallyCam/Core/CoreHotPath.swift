import Foundation
#if canImport(TallyCamCoreFFI)
    import TallyCamCoreFFI
#endif

// Per-frame calls into the Rust core (core/ffi, C ABI in
// tallycam_core_hot.h): no locks, no allocation in the core.

/// The capture host clock: `mach_absolute_time` in ns, the clock of
/// `CMClockGetHostTimeClock` and of camera and audio sample buffer times.
enum HostClock {
    static func nowNs() -> Int64 {
        lc_host_now_ns()
    }

    /// Host ticks (e.g. from `CMClockConvertHostTimeToSystemUnits`) to ns.
    static func ns(fromTicks ticks: UInt64) -> Int64 {
        lc_host_ticks_to_ns(ticks)
    }

    /// The device wall clock (never touched by the app), Unix ns.
    static func wallNowNs() -> Int64 {
        Int64(clock_gettime_nsec_np(CLOCK_REALTIME))
    }
}

/// A host instant on the disciplined clock.
struct ClockMapping: Equatable {
    /// TAI ns since the PTP epoch (1970-01-01 00:00:00 TAI).
    let taiNs: Int64
    /// UTC ns since the Unix epoch.
    let utcNs: Int64
    let synced: Bool
    /// Nil before the clock service runs.
    let estErrorNs: Int64?
    let maxErrorNs: Int64?

    /// Maps with the mapping the clock service published last. Before it
    /// runs, the device wall clock is used (not synced).
    static func at(hostNs: Int64) -> ClockMapping {
        var m = LcClockMapping()
        lc_clock_map(hostNs, &m)
        return ClockMapping(taiNs: m.tai_ns, utcNs: m.utc_ns, synced: m.synced,
                            estErrorNs: m.disciplined ? m.est_error_ns : nil,
                            maxErrorNs: m.disciplined ? m.max_error_ns : nil)
    }
}

/// The LCP capture timestamp SEI (docs/PROTOCOL.md), built by the core.
enum TimestampSei {
    enum Codec: UInt8 {
        /// nal_unit_type 6.
        case h264 = 0
        /// Prefix SEI, nal_unit_type 39.
        case hevc = 1
    }

    static let uncertaintyUnknown = UInt32.max
    static let maxNalLength = Int(LC_SEI_MAX_NAL_LEN)

    /// Microseconds, rounded up and saturating; nil is unknown.
    static func uncertaintyUs(fromNs ns: Int64?) -> UInt32 {
        lc_sei_uncertainty_us(ns ?? -1)
    }

    /// The SEI NAL unit (header and emulation-prevented RBSP, no start code).
    /// `tai`: the capture time is TAI ns since the PTP epoch (flags bit 1).
    static func nalUnit(captureNs: UInt64, synced: Bool, tai: Bool = true, frameCounter: UInt32,
                        uncertaintyUs: UInt32, codec: Codec) -> Data
    {
        let flags = UInt8((synced ? LC_SEI_FLAG_SYNCED : 0) | (tai ? LC_SEI_FLAG_TAI : 0))
        return withUnsafeTemporaryAllocation(of: UInt8.self, capacity: maxNalLength) { buffer in
            guard let base = buffer.baseAddress else {
                return Data()
            }
            let length = lc_sei_write(captureNs, flags, frameCounter, uncertaintyUs, codec.rawValue, base, buffer.count)
            return Data(bytes: base, count: length)
        }
    }
}
