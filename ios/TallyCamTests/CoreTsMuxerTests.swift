import CoreMedia
import CoreVideo
import Foundation
import XCTest
#if canImport(TallyCamCoreFFI)
    import TallyCamCoreFFI
#endif

/// The core's muxer through the Swift wrapper (`CoreTsMuxer`, lc_ts_*), fed
/// as `LiveStream` feeds it: real VideoToolbox and Apple audio encoder output
/// and synthetic frames, read back from the TS bytes. The muxer itself is
/// tested in Rust (core/src/ts, core/tests/ts_mux.rs); this checks the glue:
/// the block buffer bytes, parameter sets from the format description, the
/// SEI fields and the times. The streams are written to `$LCP_TS_DUMP_DIR`
/// (or the temporary directory) for ffprobe and GStreamer.
///
/// Before the Swift muxer was removed, the same calls through both gave
/// byte-identical output, chunk by chunk.
final class CoreTsMuxerTests: XCTestCase {
    private static let ms: Int64 = 1_000_000
    private static let sps: [UInt8] = [0x67, 0x42, 0xC0, 0x1F, 0xDA, 0x01, 0x40, 0x16, 0xEC, 0x05, 0xA8, 0x08, 0x08,
                                       0x0A, 0x00, 0x00, 0x03, 0x00, 0x02, 0x00, 0x00, 0x03, 0x00, 0x79, 0x1E, 0x30,
                                       0x65, 0x40]
    private static let pps: [UInt8] = [0x68, 0xCE, 0x3C, 0x80]
    private static let baseTai: UInt64 = 1_800_000_000_000_000_000

    /// A muxer and what it sent: one chunk per SRT send, with its content.
    private final class Recorder {
        let muxer: CoreTsMuxer
        let codec: VideoCodec
        private let box = ChunkBox()

        init(video: VideoCodec, audio: AudioCodec?) throws {
            codec = video
            let box = box
            muxer = try XCTUnwrap(CoreTsMuxer(videoCodec: video, audioCodec: audio) { data, content in
                box.chunks.append((data, content))
            })
        }

        var chunks: [(data: Data, content: UInt8)] {
            box.chunks
        }

        @discardableResult
        func video(_ buffer: CMSampleBuffer, captureNs: Int64, nowNs: Int64, counter: UInt32) -> Int32 {
            muxer.writeVideo(buffer, ptsNs: captureNs, sendCaptureNs: captureNs,
                             sei: (CoreTsMuxerTests.baseTai + UInt64(counter) * 33_333_333,
                                   UInt8(LC_SEI_FLAG_SYNCED | LC_SEI_FLAG_TAI), counter, 900),
                             nowNs: nowNs)
        }

        func dump(_ name: String) throws {
            let dir = ProcessInfo.processInfo.environment["LCP_TS_DUMP_DIR"] ?? NSTemporaryDirectory()
            let path = (dir as NSString).appendingPathComponent(name)
            try chunks.map(\.data).reduce(Data(), +).write(to: URL(fileURLWithPath: path))
            print("Core TS dump: \(path)")
        }
    }

    private final class ChunkBox {
        var chunks: [(Data, UInt8)] = []
    }

    // MARK: Synthetic frames

    /// The start rules and the frame bytes through the wrapper: nothing
    /// before the formats and the first keyframe, tables before keyframes,
    /// the slice bytes intact (non-contiguous block buffers too), the SEI
    /// with the fields given, PTS and PCR from the times given.
    func testFramesThroughTheWrapper() throws {
        let rec = try Recorder(video: .h264, audio: .opus)
        let format = try Self.h264Format()
        let t0: Int64 = 3_600_000 * Self.ms
        let key = try Self.sample(format, keyframe: true, sliceLength: 100)
        XCTAssertEqual(rec.video(key, captureNs: t0, nowNs: t0, counter: 0), Int32(LC_TS_SKIPPED), "no format")
        XCTAssertTrue(rec.muxer.setVideoFormat(format))
        XCTAssertEqual(rec.video(key, captureNs: t0, nowNs: t0, counter: 0), Int32(LC_TS_SKIPPED), "no audio format")
        XCTAssertFalse(rec.muxer.setAudioFormat(sampleRate: 44100, channels: 1))
        XCTAssertTrue(rec.muxer.setAudioFormat(sampleRate: 48000, channels: 1))
        XCTAssertEqual(rec.muxer.writeAudio(Data([1, 2]), ptsNs: t0, nowNs: t0), Int32(LC_TS_SKIPPED),
                       "audio before the first video frame")
        XCTAssertEqual(rec.video(try Self.sample(format, keyframe: false, sliceLength: 100), captureNs: t0,
                                 nowNs: t0, counter: 0), Int32(LC_TS_SKIPPED), "starts on a keyframe")
        XCTAssertTrue(rec.chunks.isEmpty)

        let lengths = [100, 183, 184, 5000, 70000]
        for (i, length) in lengths.enumerated() {
            let t = t0 + Int64(i) * 33 * Self.ms
            let buffer = try Self.sample(format, keyframe: i == 0, sliceLength: length, split: i == 3)
            XCTAssertGreaterThan(rec.video(buffer, captureNs: t, nowNs: t + 40 * Self.ms, counter: UInt32(i)), 0)
            XCTAssertGreaterThan(rec.muxer.writeAudio(Data(repeating: 7, count: 50), ptsNs: t + 2 * Self.ms,
                                                      nowNs: t + 20 * Self.ms), 0)
        }
        // A late frame is counted.
        _ = rec.video(try Self.sample(format, keyframe: false, sliceLength: 10), captureNs: t0 + 200 * Self.ms,
                      nowNs: t0 + 300 * Self.ms, counter: 5)
        XCTAssertEqual(rec.muxer.stats.lateVideoFrames, 1)
        XCTAssertEqual(rec.muxer.stats.startNs, t0)

        XCTAssertEqual(rec.chunks.map(\.content), [0, 1, 2, 1, 2, 1, 2, 1, 2, 1, 2, 1].map(UInt8.init))
        let ts = TsStream(rec.chunks.map(\.data).reduce(Data(), +))
        XCTAssertEqual(ts.videoStreamType, 0x1B)
        XCTAssertEqual(ts.video.count, 6)
        XCTAssertEqual(ts.audio.count, 5)
        for (i, frame) in ts.video.prefix(5).enumerated() {
            let t = t0 + Int64(i) * 33 * Self.ms
            XCTAssertEqual(frame.pts, Self.ticks(t))
            XCTAssertEqual(frame.pcr, Self.ticks(t + 40 * Self.ms - 80 * Self.ms), "Opus: 80 ms PCR delay")
            let nals = Self.splitAnnexB(frame.payload)
            XCTAssertEqual(nals.map { $0[0] & 0x1F }, i == 0 ? [9, 7, 8, 6, 5] : [9, 6, 1])
            if i == 0 {
                XCTAssertEqual(nals[1], Self.sps)
                XCTAssertEqual(nals[2], Self.pps)
            }
            XCTAssertEqual(nals.last, Self.slice(keyframe: i == 0, length: lengths[i]), "frame \(i)")
            let sei = try XCTUnwrap(nals.first { $0[0] == 6 })
            // The capture time and the frame counter in the SEI payload.
            let body = Self.rbsp(sei, header: 1)
            XCTAssertEqual(body[2 + 17], 0x03)
            XCTAssertEqual(body[2 + 18 ..< 2 + 26].reduce(UInt64(0)) { $0 << 8 | UInt64($1) },
                           Self.baseTai + UInt64(i) * 33_333_333)
            XCTAssertEqual(body[2 + 26 ..< 2 + 30].reduce(UInt32(0)) { $0 << 8 | UInt32($1) }, UInt32(i))
        }
        XCTAssertEqual(ts.audio.first?.pts, Self.ticks(t0 + 2 * Self.ms))
        XCTAssertEqual(Array(ts.audio[0].payload.prefix(3)), [0x7F, 0xE0, 50], "Opus control header")
    }

    // MARK: Real encoders

    func testVideoToolboxH264WithOpusMatchesTheSwiftWriter() throws {
        try encodedStream(video: .h264, audio: .opus, name: "core-h264-opus.ts")
    }

    func testVideoToolboxH264WithAacMatchesTheSwiftWriter() throws {
        try encodedStream(video: .h264, audio: .aac, name: "core-h264-aac.ts")
    }

    func testVideoToolboxHevcWithOpusMatchesTheSwiftWriter() throws {
        try encodedStream(video: .hevc, audio: .opus, name: "core-hevc-opus.ts")
    }

    /// Two seconds of VideoToolbox video (moving picture) and Apple's audio
    /// encoder (a 1 kHz tone), interleaved as on the phone; every frame
    /// read back with its SEI before the first slice.
    private func encodedStream(video: VideoCodec, audio: AudioCodec, name: String) throws {
        let frames = try encodeVideo(codec: video, count: 60)
        let packets = encodeAudio(codec: audio, seconds: 2.1)
        XCTAssertGreaterThan(packets.packets.count, 50)
        let rec = try Recorder(video: video, audio: audio)
        let format = try XCTUnwrap(frames.format)
        XCTAssertTrue(rec.muxer.setVideoFormat(format))
        XCTAssertTrue(rec.muxer.setAudioFormat(sampleRate: packets.sampleRate, channels: packets.channels))
        var audioIndex = 0
        var audioMuxed = 0
        for (index, (buffer, pts)) in frames.frames.enumerated() {
            let ns = Int64(pts.seconds * 1e9)
            XCTAssertGreaterThan(rec.video(buffer, captureNs: ns, nowNs: ns + 45 * Self.ms, counter: UInt32(index)), 0)
            while audioIndex < packets.packets.count, packets.packets[audioIndex].ptsNs < ns + 33 * Self.ms {
                let packet = packets.packets[audioIndex]
                if rec.muxer.writeAudio(packet.data, ptsNs: packet.ptsNs, nowNs: packet.ptsNs + 20 * Self.ms) > 0 {
                    audioMuxed += 1
                }
                audioIndex += 1
            }
        }
        let ts = TsStream(rec.chunks.map(\.data).reduce(Data(), +))
        XCTAssertEqual(ts.videoStreamType, video == .h264 ? 0x1B : 0x24)
        XCTAssertEqual(ts.video.count, frames.frames.count)
        XCTAssertEqual(ts.audio.count, audioMuxed)
        XCTAssertGreaterThan(audioMuxed, 50)
        for (index, frame) in ts.video.enumerated() {
            let types = Self.splitAnnexB(frame.payload).map { video == .h264 ? $0[0] & 0x1F : ($0[0] >> 1) & 0x3F }
            let (aud, sei, firstSlice): (UInt8, UInt8, Int?) = video == .h264
                ? (9, 6, types.firstIndex { (1 ... 5).contains($0) })
                : (35, 39, types.firstIndex { $0 <= 31 })
            XCTAssertEqual(types.first, aud, "AUD first in frame \(index)")
            let seiIndex = try XCTUnwrap(types.firstIndex(of: sei))
            XCTAssertLessThan(seiIndex, try XCTUnwrap(firstSlice))
            if index == 0 {
                XCTAssertEqual(Array(types[1 ..< (video == .h264 ? 3 : 4)]), video == .h264 ? [7, 8] : [32, 33, 34])
            }
            XCTAssertEqual(frame.pts, Self.ticks(Int64(frames.frames[index].1.seconds * 1e9)))
        }
        XCTAssertEqual(rec.muxer.stats.lateVideoFrames, 0)
        XCTAssertEqual(rec.muxer.stats.lateAudioFrames, 0)
        try rec.dump(name)
    }

    private func encodeVideo(codec: VideoCodec, count: Int) throws
        -> (format: CMFormatDescription?, frames: [(CMSampleBuffer, CMTime)])
    {
        let settings = VideoEncoderSettings(codec: codec, width: 640, height: 360, frameRate: 30, bitrate: 1_500_000)
        let encoder = VideoEncoder(settings: settings)
        let lock = NSLock()
        var format: CMFormatDescription?
        var frames: [(CMSampleBuffer, CMTime)] = []
        encoder.onFormat = { f in lock.withLock { format = f } }
        encoder.onOutput = { buffer, pts, _, _ in lock.withLock { frames.append((buffer, pts)) } }
        do {
            try encoder.start()
        } catch {
            throw XCTSkip("No \(codec.label) encoder here: \(error.localizedDescription)")
        }
        for i in 0 ..< count {
            encoder.encode(try Self.picture(frame: i))
        }
        encoder.stop()
        let result = lock.withLock { (format, frames) }
        XCTAssertEqual(result.1.count, count)
        return result
    }

    private func encodeAudio(codec: AudioCodec, seconds: Double)
        -> (packets: [(data: Data, ptsNs: Int64)], sampleRate: Double, channels: Int)
    {
        let encoder = AudioEncoder(codec: codec)
        var packets: [(data: Data, ptsNs: Int64)] = []
        var sampleRate = 0.0
        var channels = 0
        encoder.onFormat = { sampleRate = $0; channels = $1 }
        encoder.onOutput = { data, pts, _ in packets.append((Data(data), pts)) }
        // 33.3 s after the video's first frame time (1000 / 30 s), in 5 ms
        // buffers, as the microphone delivers them.
        let start: Int64 = 1000 * 1_000_000_000 / 30
        let total = Int(seconds * 48000)
        var samples = [Float](repeating: 0, count: 240)
        for offset in stride(from: 0, to: total, by: 240) {
            for k in 0 ..< 240 {
                samples[k] = 0.3 * sin(2 * .pi * 1000 * Float(offset + k) / 48000)
            }
            samples.withUnsafeBufferPointer {
                encoder.encode($0, captureNs: start + Int64(offset) * 1_000_000_000 / 48000, sampleRate: 48000)
            }
        }
        return (packets, sampleRate, channels)
    }

    // MARK: Helpers

    private static func h264Format() throws -> CMFormatDescription {
        var format: CMFormatDescription?
        let status = sps.withUnsafeBufferPointer { spsBuffer in
            pps.withUnsafeBufferPointer { ppsBuffer in
                CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    allocator: nil, parameterSetCount: 2,
                    parameterSetPointers: [spsBuffer.baseAddress!, ppsBuffer.baseAddress!],
                    parameterSetSizes: [sps.count, pps.count], nalUnitHeaderLength: 4, formatDescriptionOut: &format
                )
            }
        }
        XCTAssertEqual(status, noErr)
        return try XCTUnwrap(format)
    }

    /// The slice NAL unit of `sample(keyframe:sliceLength:)`.
    private static func slice(keyframe: Bool, length: Int) -> [UInt8] {
        (0 ..< length).map { i in
            i == 0 ? (keyframe ? 0x65 : 0x41) : UInt8(truncatingIfNeeded: (i + 4) &* 31 &+ length)
        }
    }

    /// One length-prefixed slice NAL unit of `sliceLength` bytes; `split`
    /// puts it in two memory blocks (a non-contiguous block buffer).
    private static func sample(_ format: CMFormatDescription, keyframe: Bool, sliceLength: Int,
                               split: Bool = false) throws -> CMSampleBuffer
    {
        let n = UInt32(sliceLength)
        let bytes = [UInt8(n >> 24), UInt8((n >> 16) & 0xFF), UInt8((n >> 8) & 0xFF), UInt8(n & 0xFF)]
            + slice(keyframe: keyframe, length: sliceLength)
        var block: CMBlockBuffer?
        let parts = split ? [bytes[..<(bytes.count / 2)], bytes[(bytes.count / 2)...]] : [bytes[...]]
        for part in parts {
            let memory = UnsafeMutableRawPointer.allocate(byteCount: part.count, alignment: 1)
            part.withUnsafeBytes { memory.copyMemory(from: $0.baseAddress!, byteCount: part.count) }
            if let existing = block {
                XCTAssertEqual(CMBlockBufferAppendMemoryBlock(
                    existing, memoryBlock: memory, length: part.count, blockAllocator: kCFAllocatorMalloc,
                    customBlockSource: nil, offsetToData: 0, dataLength: part.count, flags: 0
                ), noErr)
            } else {
                XCTAssertEqual(CMBlockBufferCreateWithMemoryBlock(
                    allocator: nil, memoryBlock: memory, blockLength: part.count, blockAllocator: kCFAllocatorMalloc,
                    customBlockSource: nil, offsetToData: 0, dataLength: part.count, flags: 0, blockBufferOut: &block
                ), noErr)
            }
        }
        var timing = CMSampleTimingInfo(duration: CMTime(value: 1, timescale: 30), presentationTimeStamp: .zero,
                                        decodeTimeStamp: .invalid)
        var size = bytes.count
        var sample: CMSampleBuffer?
        XCTAssertEqual(CMSampleBufferCreateReady(
            allocator: nil, dataBuffer: block, formatDescription: format, sampleCount: 1,
            sampleTimingEntryCount: 1, sampleTimingArray: &timing, sampleSizeEntryCount: 1,
            sampleSizeArray: &size, sampleBufferOut: &sample
        ), noErr)
        let buffer = try XCTUnwrap(sample)
        if !keyframe,
           let attachments = CMSampleBufferGetSampleAttachmentsArray(buffer, createIfNecessary: true),
           CFArrayGetCount(attachments) > 0
        {
            let dict = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(dict,
                                 Unmanaged.passUnretained(kCMSampleAttachmentKey_NotSync).toOpaque(),
                                 Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
        }
        return buffer
    }

    /// A moving gradient, so the encoder makes real P frames.
    private static func picture(frame: Int) throws -> CMSampleBuffer {
        var pixelBuffer: CVPixelBuffer?
        let attributes = [kCVPixelBufferIOSurfacePropertiesKey: [:]] as CFDictionary
        XCTAssertEqual(CVPixelBufferCreate(nil, 640, 360, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                                           attributes, &pixelBuffer), kCVReturnSuccess)
        let buffer = try XCTUnwrap(pixelBuffer)
        CVPixelBufferLockBaseAddress(buffer, [])
        for plane in 0 ..< CVPixelBufferGetPlaneCount(buffer) {
            let base = try XCTUnwrap(CVPixelBufferGetBaseAddressOfPlane(buffer, plane)).assumingMemoryBound(to: UInt8.self)
            let row = CVPixelBufferGetBytesPerRowOfPlane(buffer, plane)
            let height = CVPixelBufferGetHeightOfPlane(buffer, plane)
            for y in 0 ..< height {
                for x in 0 ..< row {
                    base[y * row + x] = plane == 0 ? UInt8(truncatingIfNeeded: x + y + frame * 4) : 128
                }
            }
        }
        CVPixelBufferUnlockBaseAddress(buffer, [])
        var format: CMVideoFormatDescription?
        CMVideoFormatDescriptionCreateForImageBuffer(allocator: nil, imageBuffer: buffer, formatDescriptionOut: &format)
        var timing = CMSampleTimingInfo(duration: CMTime(value: 1, timescale: 30),
                                        presentationTimeStamp: CMTime(value: CMTimeValue(1000 + frame), timescale: 30),
                                        decodeTimeStamp: .invalid)
        var sample: CMSampleBuffer?
        CMSampleBufferCreateReadyWithImageBuffer(allocator: nil, imageBuffer: buffer,
                                                 formatDescription: try XCTUnwrap(format),
                                                 sampleTiming: &timing, sampleBufferOut: &sample)
        return try XCTUnwrap(sample)
    }

    /// Host ns to 90 kHz ticks, as the muxer does.
    private static func ticks(_ ns: Int64) -> Int64 {
        (ns / 100_000 * 9 + (ns % 100_000) * 9 / 100_000) & ((1 << 33) - 1)
    }

    private static func rbsp(_ nal: [UInt8], header: Int) -> [UInt8] {
        var out: [UInt8] = []
        var zeros = 0
        for byte in nal.dropFirst(header) {
            if zeros >= 2, byte == 0x03 {
                zeros = 0
                continue
            }
            out.append(byte)
            zeros = byte == 0 ? zeros + 1 : 0
        }
        return out
    }

    /// NAL units of an Annex B stream, without start codes.
    private static func splitAnnexB(_ bytes: [UInt8]) -> [[UInt8]] {
        var starts: [(code: Int, payload: Int)] = []
        var i = 0
        while i + 2 < bytes.count {
            if bytes[i] == 0, bytes[i + 1] == 0, bytes[i + 2] == 1 {
                let code = i > 0 && bytes[i - 1] == 0 ? i - 1 : i
                starts.append((code, i + 3))
                i += 3
            } else {
                i += 1
            }
        }
        return starts.enumerated().map { index, start in
            let end = index + 1 < starts.count ? starts[index + 1].code : bytes.count
            return Array(bytes[start.payload ..< end])
        }
    }

    /// A minimal reader of the muxer's output: the PMT's video stream type
    /// and each PES on the video (256) and audio (257) PIDs.
    private struct TsStream {
        struct Pes {
            var pts: Int64
            /// PCR in the PES's first packet, 90 kHz base.
            var pcr: Int64?
            var payload: [UInt8]
        }

        var videoStreamType: UInt8?
        var video: [Pes] = []
        var audio: [Pes] = []

        init(_ data: Data) {
            let ts = [UInt8](data)
            XCTAssertEqual(ts.count % 188, 0)
            var ccs: [UInt16: UInt8] = [:]
            for offset in stride(from: 0, to: ts.count, by: 188) {
                let p = Array(ts[offset ..< offset + 188])
                XCTAssertEqual(p[0], 0x47)
                let pid = UInt16(p[1] & 0x1F) << 8 | UInt16(p[2])
                let cc = p[3] & 0x0F
                if let last = ccs[pid] {
                    XCTAssertEqual(cc, (last + 1) & 0x0F, "continuity on PID \(pid)")
                }
                ccs[pid] = cc
                let pusi = p[1] & 0x40 != 0
                var i = 4
                var pcr: Int64?
                if p[3] & 0x20 != 0 {
                    if p[4] > 0, p[5] & 0x10 != 0 {
                        pcr = Int64(p[6]) << 25 | Int64(p[7]) << 17 | Int64(p[8]) << 9 | Int64(p[9]) << 1
                            | Int64(p[10]) >> 7
                    }
                    i = 5 + Int(p[4])
                }
                if pid == 4095 {
                    videoStreamType = p[i + 1 + 12]
                    continue
                }
                guard pid == 256 || pid == 257 else {
                    continue
                }
                if pusi {
                    let header = 9 + Int(p[i + 8])
                    let b = p[(i + 9) ..< (i + 14)].map(Int64.init)
                    let pts = (b[0] >> 1 & 7) << 30 | b[1] << 22 | (b[2] >> 1) << 15 | b[3] << 7 | b[4] >> 1
                    let pes = Pes(pts: pts, pcr: pcr, payload: Array(p[(i + header)...]))
                    if pid == 256 {
                        video.append(pes)
                    } else {
                        audio.append(pes)
                    }
                } else if pid == 256 {
                    video[video.count - 1].payload += p[i...]
                } else {
                    audio[audio.count - 1].payload += p[i...]
                }
            }
        }
    }
}
