import CoreMedia
import Foundation
#if canImport(TallyCamCoreFFI)
    import TallyCamCoreFFI
#endif

/// The core's MPEG-TS muxer (`lc_ts_*` in tallycam_core_hot.h): encoded
/// frames go in with their host times, and the core builds the access unit
/// (AUD, parameter sets, timestamp SEI), muxes it and queues the packets on
/// the SRT caller itself. Not thread-safe: call every method on the queue
/// that owns the SRT connection.
final class CoreTsMuxer {
    /// What a write did: not muxed yet (no format, or waiting for the first
    /// keyframe), or the SRT result (payloads queued, or an `LC_SRT_*` code).
    typealias Result = Int32

    struct Stats {
        var lateVideoFrames: Int
        var lateAudioFrames: Int
        /// Host ns of the first muxed video frame.
        var startNs: Int64?
    }

    private let mux: OpaquePointer
    private let codec: VideoCodec
    /// Frame bytes when a block buffer is not contiguous (rare); reused.
    private var scratch: [UInt8] = []
    /// For `init(videoCodec:audioCodec:output:)`.
    private var output: OutputBox?

    /// Sends on the SRT caller behind `srtHandle` (`SrtCaller.hotHandle()`).
    /// The muxer keeps that caller's sender alive.
    init?(srtHandle: UInt64, videoCodec: VideoCodec, audioCodec: AudioCodec?) {
        guard let mux = lc_ts_new(srtHandle, videoCodec.coreCode, audioCodec?.coreCode ?? UInt8(LC_TS_AUDIO_NONE), 0)
        else {
            return nil
        }
        self.mux = mux
        codec = videoCodec
    }

    /// Hands the packets to `output` instead (tests): bytes, `LC_TS_CONTENT_*`.
    convenience init?(videoCodec: VideoCodec, audioCodec: AudioCodec?, output: @escaping (Data, UInt8) -> Void) {
        self.init(srtHandle: 0, videoCodec: videoCodec, audioCodec: audioCodec)
        let box = OutputBox(output)
        self.output = box
        lc_ts_set_output(mux, { ctx, data, len, content, _ in
            guard let ctx, let data else {
                return -4
            }
            Unmanaged<OutputBox>.fromOpaque(ctx).takeUnretainedValue().output(Data(bytes: data, count: len), content)
            return Int32((len + 1315) / 1316)
        }, Unmanaged.passUnretained(box).toOpaque())
    }

    deinit {
        lc_ts_free(mux)
    }

    /// From the encoder's format description: the parameter sets. False if
    /// they are missing (video is then not muxed).
    @discardableResult
    func setVideoFormat(_ format: CMFormatDescription) -> Bool {
        guard let (sets, nalLengthSize) = EncoderParameterSets.annexB(format, codec: codec) else {
            lc_ts_set_video_format(mux, nil, 0, 4)
            return false
        }
        return sets.withUnsafeBytes { buffer in
            lc_ts_set_video_format(mux, buffer.bindMemory(to: UInt8.self).baseAddress, buffer.count,
                                   UInt8(clamping: nalLengthSize))
        }
    }

    @discardableResult
    func setAudioFormat(sampleRate: Double, channels: Int) -> Bool {
        lc_ts_set_audio_format(mux, UInt32(clamping: Int(sampleRate)), UInt8(clamping: channels))
    }

    /// - Parameters:
    ///   - ptsNs: Capture time of the frame on the host clock (calibrated).
    ///   - sendCaptureNs: Host ns reported for the frame by
    ///     `SrtConnection.onFrameSent`.
    ///   - sei: The timestamp SEI fields; nil for none.
    func writeVideo(_ sampleBuffer: CMSampleBuffer, ptsNs: Int64, sendCaptureNs: Int64,
                    sei: (captureNs: UInt64, flags: UInt8, frameCounter: UInt32, uncertaintyUs: UInt32)?,
                    nowNs: Int64) -> Result
    {
        guard let dataBuffer = sampleBuffer.dataBuffer else {
            return Result(LC_TS_SKIPPED)
        }
        var frame = LcTsVideoFrame()
        frame.keyframe = sampleBuffer.isSyncSample
        frame.pts_ns = ptsNs
        frame.send_capture_ns = sendCaptureNs
        if let sei {
            frame.has_sei = true
            frame.sei_capture_ns = sei.captureNs
            frame.sei_flags = sei.flags
            frame.sei_frame_counter = sei.frameCounter
            frame.sei_uncertainty_us = sei.uncertaintyUs
        }
        var lengthAtOffset = 0
        var total = 0
        var pointer: UnsafeMutablePointer<CChar>?
        if CMBlockBufferGetDataPointer(dataBuffer, atOffset: 0, lengthAtOffsetOut: &lengthAtOffset,
                                       totalLengthOut: &total, dataPointerOut: &pointer) == kCMBlockBufferNoErr,
            let pointer, lengthAtOffset == total
        {
            // VideoToolbox output is contiguous: no copy.
            return pointer.withMemoryRebound(to: UInt8.self, capacity: total) { bytes in
                frame.data = UnsafePointer(bytes)
                frame.len = total
                return lc_ts_video(mux, &frame, nowNs)
            }
        }
        let length = CMBlockBufferGetDataLength(dataBuffer)
        if scratch.count < length {
            scratch = [UInt8](repeating: 0, count: length)
        }
        return scratch.withUnsafeMutableBytes { buffer in
            guard let base = buffer.baseAddress,
                  CMBlockBufferCopyDataBytes(dataBuffer, atOffset: 0, dataLength: length,
                                             destination: base) == kCMBlockBufferNoErr
            else {
                return Result(LC_TS_SKIPPED)
            }
            frame.data = UnsafePointer(base.assumingMemoryBound(to: UInt8.self))
            frame.len = length
            return lc_ts_video(mux, &frame, nowNs)
        }
    }

    /// - Parameters:
    ///   - frame: One Opus packet, or one raw AAC frame without ADTS header.
    ///   - ptsNs: Capture time of the first sample minus the encoder delay.
    func writeAudio(_ frame: Data, ptsNs: Int64, nowNs: Int64) -> Result {
        frame.withUnsafeBytes { buffer in
            lc_ts_audio(mux, buffer.bindMemory(to: UInt8.self).baseAddress, buffer.count, ptsNs, nowNs)
        }
    }

    var stats: Stats {
        var s = LcTsStats()
        lc_ts_stats(mux, &s)
        return Stats(lateVideoFrames: Int(s.late_video_frames), lateAudioFrames: Int(s.late_audio_frames),
                     startNs: s.started ? s.start_ns : nil)
    }

    private final class OutputBox {
        let output: (Data, UInt8) -> Void

        init(_ output: @escaping (Data, UInt8) -> Void) {
            self.output = output
        }
    }
}

/// The parameter sets of an encoder's format description, for the muxer.
enum EncoderParameterSets {
    /// Annex B (each set after a 4-byte start code) and the length prefix
    /// size of the frames. H.264: SPS and PPS (the first two sets); HEVC:
    /// every set (VPS, SPS, PPS and any other). Nil if they are missing.
    static func annexB(_ format: CMFormatDescription, codec: VideoCodec) -> (Data, Int)? {
        typealias Getter = (CMFormatDescription, Int, UnsafeMutablePointer<UnsafePointer<UInt8>?>?,
                            UnsafeMutablePointer<Int>?, UnsafeMutablePointer<Int>?,
                            UnsafeMutablePointer<Int32>?) -> OSStatus
        let get: Getter = switch codec {
        case .h264: CMVideoFormatDescriptionGetH264ParameterSetAtIndex
        case .hevc: CMVideoFormatDescriptionGetHEVCParameterSetAtIndex
        }
        var count = 0
        var nalLengthSize: Int32 = 0
        guard get(format, 0, nil, nil, &count, &nalLengthSize) == noErr,
              count >= (codec == .h264 ? 2 : 3)
        else {
            return nil
        }
        var out = Data()
        for index in 0 ..< (codec == .h264 ? 2 : count) {
            var pointer: UnsafePointer<UInt8>?
            var size = 0
            guard get(format, index, &pointer, &size, nil, nil) == noErr, let pointer, size > 0 else {
                return nil
            }
            out.append(contentsOf: [0, 0, 0, 1])
            out.append(pointer, count: size)
        }
        return (out, Int(nalLengthSize))
    }
}

extension CMSampleBuffer {
    /// A sync sample (keyframe): not marked NotSync.
    var isSyncSample: Bool {
        guard let attachments = CMSampleBufferGetSampleAttachmentsArray(self, createIfNecessary: false)
            as? [[CFString: Any]],
            let first = attachments.first
        else {
            return true
        }
        return !(first[kCMSampleAttachmentKey_NotSync] as? Bool ?? false)
    }
}

extension VideoCodec {
    var coreCode: UInt8 {
        switch self {
        case .h264: UInt8(LC_TS_VIDEO_H264)
        case .hevc: UInt8(LC_TS_VIDEO_HEVC)
        }
    }
}

extension AudioCodec {
    var coreCode: UInt8 {
        switch self {
        case .opus: UInt8(LC_TS_AUDIO_OPUS)
        case .aac: UInt8(LC_TS_AUDIO_AAC)
        }
    }
}
