import Foundation

/// How microphone samples get their capture time, and how an encoded audio
/// frame gets its PTS. All times are host clock ns (`HostClock`, the clock
/// of the camera's sample buffer times), so audio and video PTS are on one
/// timescale.
enum AudioTiming {
    /// Capture time of the first sample of a microphone input buffer:
    ///
    ///     capture_ns = host_ns(AudioTimeStamp.mHostTime) - AVAudioSession.inputLatency
    ///
    /// `mHostTime` is the host time Core Audio gives the buffer's first
    /// sample at the I/O unit; `inputLatency` is the hardware and driver
    /// latency before that (converter, ADC, safety offset), so the result is
    /// when the sound reached the microphone.
    static func captureNs(bufferHostNs: Int64, inputLatency: TimeInterval) -> Int64 {
        bufferHostNs - Int64((max(inputLatency, 0) * 1e9).rounded())
    }

    /// Duration of `frames` samples, ns (rounded down).
    static func durationNs(frames: Int, sampleRate: Double) -> Int64 {
        Int64((Double(frames) * 1e9 / sampleRate).rounded(.down))
    }

    /// PTS of an encoded frame, host ns: the capture time of the frame's
    /// first input sample, moved back by the encoder delay. A decoder in
    /// MPEG-TS outputs `primingFrames` samples of delay before the frame's
    /// own first sample (Opus pre-skip, AAC priming) and does not skip them,
    /// so its output sample 0 is the input sample `-primingFrames`; with
    /// this PTS every decoded sample lands at its capture time.
    static func ptsNs(firstSampleNs: Int64, primingFrames: Int, sampleRate: Double) -> Int64 {
        firstSampleNs - durationNs(frames: primingFrames, sampleRate: sampleRate)
    }
}

/// Cuts microphone buffers (any length) into encoder frames of
/// `frameLength` samples and stamps each frame with the capture time of its
/// first sample, interpolated within the buffer it starts in. Each frame is
/// stamped from its own buffer's capture time, so the timestamps follow the
/// microphone's clock against the host clock without drift.
///
/// A buffer whose capture time is off by more than `maxGapNs` from where the
/// previous one ended (dropped buffers, an interruption) discards the
/// partial frame, so a frame never spans a gap.
struct AudioFramer {
    let frameLength: Int
    let sampleRate: Double
    let maxGapNs: Int64
    /// Partial frames discarded because of a gap.
    private(set) var discontinuities = 0
    private var frame: [Float]
    private var filled = 0
    private var frameStartNs: Int64 = 0
    private var expectedNs: Int64?

    init(frameLength: Int, sampleRate: Double, maxGapNs: Int64 = 2_000_000) {
        self.frameLength = frameLength
        self.sampleRate = sampleRate
        self.maxGapNs = maxGapNs
        frame = [Float](repeating: 0, count: frameLength)
    }

    /// `emit` gets each completed frame and the capture time of its first
    /// sample. The frame's memory is only valid during the call.
    mutating func push(_ samples: UnsafeBufferPointer<Float>, captureNs: Int64,
                       emit: (UnsafeBufferPointer<Float>, Int64) -> Void)
    {
        if let expectedNs, abs(captureNs - expectedNs) > maxGapNs, filled > 0 {
            filled = 0
            discontinuities += 1
        }
        expectedNs = captureNs + AudioTiming.durationNs(frames: samples.count, sampleRate: sampleRate)
        var index = 0
        while index < samples.count {
            if filled == 0 {
                frameStartNs = captureNs + AudioTiming.durationNs(frames: index, sampleRate: sampleRate)
            }
            let n = min(frameLength - filled, samples.count - index)
            frame.withUnsafeMutableBufferPointer { destination in
                for i in 0 ..< n {
                    destination[filled + i] = samples[index + i]
                }
            }
            filled += n
            index += n
            if filled == frameLength {
                frame.withUnsafeBufferPointer { emit($0, frameStartNs) }
                filled = 0
            }
        }
    }

    /// Drops a partial frame (the stream restarts).
    mutating func reset() {
        filled = 0
        expectedNs = nil
    }
}
