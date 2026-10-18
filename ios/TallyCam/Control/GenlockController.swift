@preconcurrency import AVFoundation
import os

private let log = Logger(subsystem: "se.eyevinn.tallycam", category: "genlock")

/// Soft genlock on the capture path. Every frame's capture time, mapped to
/// TAI by the clock discipline, goes to the core loop on the video queue;
/// the frame duration it asks for about once a second is applied on the
/// session queue (`apply`). Runs whether or not the stream is live, so the
/// camera is on the grid before it goes live.
final class GenlockController: @unchecked Sendable {
    struct Config: Equatable {
        var fps: Int
        /// The active format's shortest frame, ns.
        var minFrameDurationNs: Double
    }

    private static let enabledKey = "genlockEnabled"
    private static let offsetKey = "genlockOffsetUs"
    /// The advertised range of `genlock_offset_us`. Also keeps the ns
    /// conversion in `onFrame` far from overflowing.
    static let offsetRangeUs: ClosedRange<Double> = -50000 ... 50000

    /// Called on the video queue with a frame duration (ns) to apply and the
    /// loop's serial: apply it only while `isCurrent(serial:)`.
    var apply: (@Sendable (Double, Int) -> Void)?
    /// Called on the video queue when the loop's state changes.
    var onStateChange: (@Sendable (GenlockLoop.State, GenlockLoop.Status) -> Void)?

    private let lock = NSLock()
    // Guarded by `lock`.
    private var enabled: Bool
    private var config: Config?
    private var configSerial = 0
    private var latest: GenlockLoop.Status?
    private var offsetUs: Double

    // Video queue only.
    private var loop: GenlockLoop?
    private var loopSerial = -1
    private var appliedOffsetUs: Double?
    private var lastState: GenlockLoop.State?

    init() {
        enabled = UserDefaults.standard.bool(forKey: Self.enabledKey)
        offsetUs = Self.clampedOffset(UserDefaults.standard.double(forKey: Self.offsetKey))
    }

    /// Within `offsetRangeUs`; not finite is 0.
    static func clampedOffset(_ us: Double) -> Double {
        us.isFinite ? min(max(us, offsetRangeUs.lowerBound), offsetRangeUs.upperBound) : 0
    }

    /// Where frames land after each grid point, us: the calibration of this
    /// model's capture stamp against the light it records.
    var targetOffsetUs: Double {
        lock.withLock { offsetUs }
    }

    func setTargetOffset(us: Double) {
        let us = Self.clampedOffset(us)
        lock.withLock { offsetUs = us }
        UserDefaults.standard.set(us, forKey: Self.offsetKey)
    }

    var isEnabled: Bool {
        lock.withLock { enabled }
    }

    func setEnabled(_ on: Bool) {
        lock.withLock {
            enabled = on
            configSerial += 1
            if !on {
                latest = nil
            }
        }
        UserDefaults.standard.set(on, forKey: Self.enabledKey)
        log.info("Genlock \(on ? "on" : "off", privacy: .public)")
    }

    /// The format or frame rate changed: the loop starts over.
    func configure(_ config: Config) {
        lock.withLock {
            guard config != self.config else {
                return
            }
            self.config = config
            configSerial += 1
            latest = nil
        }
    }

    /// Another device or format with the same config (a lens switch at the
    /// same frame rate): the loop starts over, and durations queued for the
    /// old one are dropped.
    func restart() {
        lock.withLock {
            configSerial += 1
            latest = nil
        }
    }

    /// True while genlock is on and the loop with `serial` is still the
    /// current one (no format or frame rate change since).
    func isCurrent(serial: Int) -> Bool {
        lock.withLock { enabled && serial == configSerial }
    }

    /// Latest status while enabled.
    var status: GenlockLoop.Status? {
        lock.withLock { latest }
    }

    /// Every captured frame, on the video queue.
    func onFrame(_ sampleBuffer: CMSampleBuffer) {
        let (on, config, serial, offset) = lock.withLock { (enabled, config, configSerial, offsetUs) }
        guard on, let config else {
            loop = nil
            lastState = nil
            return
        }
        if serial != loopSerial {
            loopSerial = serial
            lastState = nil
            loop = GenlockLoop(fps: config.fps, minFrameDurationNs: config.minFrameDurationNs)
            appliedOffsetUs = nil
            log.info("Genlock loop at \(config.fps) fps")
        }
        if offset != appliedOffsetUs {
            appliedOffsetUs = offset
            loop?.setTargetOffset(ns: Int64((offset * 1000).rounded()))
        }
        let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        guard let loop, pts.isValid else {
            return
        }
        let hostNs = HostClock.ns(fromTicks: CMClockConvertHostTimeToSystemUnits(pts))
        // Lock the light, not the stamp: the same correction as the SEI.
        let mapping = ClockMapping.at(hostNs: CaptureCalibration.exposureNs(stampNs: hostNs))
        guard let duration = loop.onFrame(captureTaiNs: mapping.taiNs, synced: mapping.synced) else {
            return
        }
        let status = loop.status
        lock.withLock {
            if serial == configSerial {
                latest = status
            }
        }
        apply?(duration, serial)
        if status.state != lastState {
            lastState = status.state
            onStateChange?(status.state, status)
        }
    }
}

extension GenlockLoop.Status {
    /// `status.genlock` (docs/PROTOCOL.md).
    var message: StatusMessage.Genlock {
        StatusMessage.Genlock(state: state.rawValue,
                              phaseErrorMs: (phaseErrorMs * 1000).rounded() / 1000,
                              phaseErrorMaxMs: (phaseErrorMaxMs * 1000).rounded() / 1000,
                              correctionPpm: (correctionPpm * 10).rounded() / 10,
                              frameDurationUs: (frameDurationNs / 10).rounded() / 100)
    }
}
