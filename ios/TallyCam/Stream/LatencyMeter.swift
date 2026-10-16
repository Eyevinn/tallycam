import Foundation

/// Per-frame latency of the send pipeline, aggregated to p50/p95/max once a
/// second. Every stamp is a host clock time in ns (`HostClock.nowNs()`, the
/// clock of capture timestamps).
///
/// `record` stores into a preallocated buffer: no allocation, no locks. Use
/// it from one serial queue (the live stream's queue). `report()` allocates
/// and sorts, once a second.
struct LatencyMeter {
    enum Stage: Int, CaseIterable {
        /// Camera capture time to the frame being handed to the encoder.
        case captureToEncode
        /// Encoder input to the encoder's output callback.
        case encode
        /// Encoder output to MPEG-TS packets ready (queue hop, SEI, PES).
        case mux
        /// TS packets ready to the first SRT packet of the frame on the wire.
        case sendQueue
        /// TS packets ready to the last SRT packet of the frame on the wire.
        case sendFrame
        /// Camera capture time to the first SRT packet of the frame on the wire.
        case captureToSend
        /// Capture time of the first sample of a microphone buffer to the
        /// buffer reaching the audio encoder (input latency, buffer length
        /// and delivery).
        case audioCaptureToEncode
        /// A frame's last sample handed to the audio encoder to its packet.
        case audioEncode
        /// Audio PTS (capture time of the frame's first sample minus the
        /// encoder delay) to its TS packets.
        case audioCaptureToMux
    }

    /// Samples per stage per window; more are dropped until the next report.
    static let capacity = 256
    private var samples: ContiguousArray<Int32>
    private var counts: ContiguousArray<Int>

    init() {
        samples = ContiguousArray(repeating: 0, count: Stage.allCases.count * Self.capacity)
        counts = ContiguousArray(repeating: 0, count: Stage.allCases.count)
    }

    /// Records a duration in ns. Negative durations (clock confusion) are
    /// recorded as 0.
    mutating func record(_ stage: Stage, ns: Int64) {
        let i = stage.rawValue
        let n = counts[i]
        guard n < Self.capacity else {
            return
        }
        let us = Int32(clamping: max(ns, 0) / 1000)
        samples[i * Self.capacity + n] = us
        counts[i] = n + 1
    }

    /// p50/p95/max of each stage since the last report, then starts a new window.
    mutating func report() -> LatencyReport {
        var report = LatencyReport()
        for stage in Stage.allCases {
            let i = stage.rawValue
            let n = counts[i]
            counts[i] = 0
            guard n > 0 else {
                continue
            }
            let window = samples[i * Self.capacity ..< i * Self.capacity + n].sorted()
            let value = LatencyReport.Percentiles(sortedMicroseconds: window)
            switch stage {
            case .captureToEncode: report.captureToEncodeMs = value
            case .encode: report.encodeMs = value
            case .mux: report.muxMs = value
            case .sendQueue: report.sendQueueMs = value
            case .sendFrame: report.sendFrameMs = value
            case .captureToSend: report.captureToSendMs = value
            case .audioCaptureToEncode: report.audioCaptureToEncodeMs = value
            case .audioEncode: report.audioEncodeMs = value
            case .audioCaptureToMux: report.audioCaptureToMuxMs = value
            }
        }
        return report
    }
}

/// `status.stream.latency`: p50/p95/max per stage, ms. See docs/PROTOCOL.md.
struct LatencyReport: Encodable, Equatable {
    struct Percentiles: Encodable, Equatable {
        let p50: Double
        let p95: Double
        let max: Double

        init(p50: Double, p95: Double, max: Double) {
            self.p50 = p50
            self.p95 = p95
            self.max = max
        }

        init(sortedMicroseconds v: [Int32]) {
            func ms(_ us: Int32) -> Double {
                (Double(us) / 100).rounded() / 10
            }
            func at(_ p: Double) -> Int32 {
                v[Int((Double(v.count - 1) * p).rounded())]
            }
            p50 = ms(at(0.5))
            p95 = ms(at(0.95))
            max = ms(v[v.count - 1])
        }
    }

    var captureToEncodeMs: Percentiles?
    var encodeMs: Percentiles?
    var muxMs: Percentiles?
    var sendQueueMs: Percentiles?
    var sendFrameMs: Percentiles?
    var captureToSendMs: Percentiles?
    var audioCaptureToEncodeMs: Percentiles?
    var audioEncodeMs: Percentiles?
    var audioCaptureToMuxMs: Percentiles?
}
