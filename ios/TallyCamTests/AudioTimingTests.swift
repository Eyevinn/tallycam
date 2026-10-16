import Foundation
import XCTest

/// Microphone timestamps, framing and the audio PTS (docs/LATENCY.md,
/// "Audio timestamps").
final class AudioTimingTests: XCTestCase {
    private static let ms: Int64 = 1_000_000

    // MARK: Timestamp formula

    func testCaptureTimeIsBufferHostTimeMinusInputLatency() {
        XCTAssertEqual(AudioTiming.captureNs(bufferHostNs: 5_000 * Self.ms, inputLatency: 0.0021),
                       5_000 * Self.ms - 2_100_000)
        XCTAssertEqual(AudioTiming.captureNs(bufferHostNs: 5_000 * Self.ms, inputLatency: 0), 5_000 * Self.ms)
        XCTAssertEqual(AudioTiming.captureNs(bufferHostNs: 5_000 * Self.ms, inputLatency: -1), 5_000 * Self.ms,
                       "a negative latency is ignored")
    }

    func testDurations() {
        XCTAssertEqual(AudioTiming.durationNs(frames: 480, sampleRate: 48000), 10 * Self.ms)
        XCTAssertEqual(AudioTiming.durationNs(frames: 1, sampleRate: 48000), 20833)
        XCTAssertEqual(AudioTiming.durationNs(frames: 1024, sampleRate: 48000), 21_333_333)
    }

    // MARK: PTS and encoder delay

    func testOpusPtsIsFirstSampleMinusPreSkip() {
        let first: Int64 = 9_000 * Self.ms
        // 312 samples of pre-skip at 48 kHz: 6.5 ms.
        XCTAssertEqual(AudioTiming.ptsNs(firstSampleNs: first, primingFrames: AudioCodec.opus.defaultPrimingFrames,
                                         sampleRate: 48000), first - 6_500_000)
        // A decoder that does not skip the pre-skip outputs input sample k of
        // the frame as its sample 312 + k: at PTS + (312 + k) / 48 kHz, the
        // capture time of sample k.
        let pts = AudioTiming.ptsNs(firstSampleNs: first, primingFrames: 312, sampleRate: 48000)
        let k = 100
        XCTAssertEqual(pts + AudioTiming.durationNs(frames: 312 + k, sampleRate: 48000),
                       first + AudioTiming.durationNs(frames: k, sampleRate: 48000), accuracy: 1)
    }

    func testAacPtsIsFirstSampleMinusPriming() {
        XCTAssertEqual(AudioTiming.ptsNs(firstSampleNs: 100 * Self.ms, primingFrames: AudioCodec.aac.defaultPrimingFrames,
                                         sampleRate: 48000), 100 * Self.ms - 44 * Self.ms)
    }

    // MARK: Framing

    /// 5 ms microphone buffers into 10 ms Opus frames: every frame starts on
    /// a buffer, stamped with that buffer's capture time.
    func testFramesOfTwoBuffers() {
        var framer = AudioFramer(frameLength: 480, sampleRate: 48000)
        var stamps: [Int64] = []
        var firstSamples: [Float] = []
        let t0: Int64 = 1_000 * Self.ms
        for k in 0 ..< 6 {
            let samples = (0 ..< 240).map { Float(k * 240 + $0) }
            samples.withUnsafeBufferPointer { buffer in
                framer.push(buffer, captureNs: t0 + Int64(k) * 5 * Self.ms) { frame, ns in
                    XCTAssertEqual(frame.count, 480)
                    stamps.append(ns)
                    firstSamples.append(frame[0])
                }
            }
        }
        XCTAssertEqual(stamps, [t0, t0 + 10 * Self.ms, t0 + 20 * Self.ms])
        XCTAssertEqual(firstSamples, [0, 480, 960])
    }

    /// A frame that starts inside a buffer is stamped at its offset in it.
    func testFrameStartingInsideABuffer() {
        var framer = AudioFramer(frameLength: 480, sampleRate: 48000)
        var stamps: [Int64] = []
        let t0: Int64 = 2_000 * Self.ms
        let chunk = 256
        for k in 0 ..< 4 {
            let samples = [Float](repeating: 0, count: chunk)
            samples.withUnsafeBufferPointer { buffer in
                framer.push(buffer, captureNs: t0 + AudioTiming.durationNs(frames: k * chunk, sampleRate: 48000)) { _, ns in
                    stamps.append(ns)
                }
            }
        }
        // Frame 1 starts at sample 480: buffer 1 (from 256) at offset 224.
        XCTAssertEqual(stamps.count, 2)
        XCTAssertEqual(stamps[0], t0)
        XCTAssertEqual(stamps[1], t0 + AudioTiming.durationNs(frames: 256, sampleRate: 48000)
            + AudioTiming.durationNs(frames: 224, sampleRate: 48000))
        XCTAssertEqual(stamps[1], t0 + 10 * Self.ms, accuracy: 1)
    }

    /// The stamps come from the buffers, not from counting samples: a
    /// microphone clock that runs fast against the host clock does not
    /// drift away from the capture times.
    func testStampsFollowTheBufferTimes() {
        var framer = AudioFramer(frameLength: 480, sampleRate: 48000)
        var stamps: [Int64] = []
        // Each 5 ms buffer arrives 5.001 ms after the previous one.
        for k in 0 ..< 2000 {
            let samples = [Float](repeating: 0, count: 240)
            samples.withUnsafeBufferPointer { buffer in
                framer.push(buffer, captureNs: Int64(k) * 5_001_000) { _, ns in stamps.append(ns) }
            }
        }
        XCTAssertEqual(stamps.last, 1998 * 5_001_000)
        XCTAssertEqual(framer.discontinuities, 0)
    }

    /// A missing buffer: the partial frame is dropped, the next frame starts
    /// on the first buffer after the gap.
    func testGapDropsThePartialFrame() {
        var framer = AudioFramer(frameLength: 480, sampleRate: 48000)
        var stamps: [Int64] = []
        let t0: Int64 = 3_000 * Self.ms
        for k in [0, 1, 2, 4, 5] {
            let samples = [Float](repeating: 0, count: 240)
            samples.withUnsafeBufferPointer { buffer in
                framer.push(buffer, captureNs: t0 + Int64(k) * 5 * Self.ms) { _, ns in stamps.append(ns) }
            }
        }
        XCTAssertEqual(stamps, [t0, t0 + 20 * Self.ms])
        XCTAssertEqual(framer.discontinuities, 1)
    }
}

/// The encoder with Apple's real Opus and AAC encoders: one packet per
/// frame, PTS = first sample's capture time minus the encoder delay.
final class AudioEncoderTests: XCTestCase {
    private func encode(_ codec: AudioCodec, buffers: Int) -> [(Data, Int64)] {
        let encoder = AudioEncoder(codec: codec)
        var packets: [(Data, Int64)] = []
        var format: (Double, Int)?
        encoder.onFormat = { format = ($0, $1) }
        encoder.onOutput = { packet, pts, _ in packets.append((packet, pts)) }
        let t0: Int64 = 50_000_000_000
        for k in 0 ..< buffers {
            let samples = (0 ..< 240).map { Float(sin(Double(k * 240 + $0) * 0.13) * 0.5) }
            samples.withUnsafeBufferPointer {
                encoder.encode($0, captureNs: t0 + Int64(k) * 5_000_000, sampleRate: 48000)
            }
        }
        XCTAssertEqual(format?.0, 48000)
        XCTAssertEqual(format?.1, 1)
        return packets
    }

    func testOpusOnePacketPerTenMilliseconds() throws {
        let packets = encode(.opus, buffers: 20)
        XCTAssertEqual(packets.count, 10, "no frame held back")
        let t0: Int64 = 50_000_000_000
        for (n, packet) in packets.enumerated() {
            XCTAssertEqual(packet.1, t0 + Int64(n) * 10_000_000 - 6_500_000, "packet \(n)")
            XCTAssertGreaterThan(packet.0.count, 0)
        }
    }

    func testAacPtsMovedBackByPriming() throws {
        let packets = encode(.aac, buffers: 64)
        let first = try XCTUnwrap(packets.first)
        let t0: Int64 = 50_000_000_000
        XCTAssertEqual(first.1, t0 - 44_000_000)
        if packets.count > 1 {
            XCTAssertEqual(packets[1].1 - packets[0].1, 21_333_333, accuracy: 1)
        }
    }
}
