@preconcurrency import CoreMedia
import Foundation
import os
@preconcurrency import VideoToolbox

private let log = Logger(subsystem: "se.eyevinn.tallycam", category: "video-encoder")

/// The video codec of the stream, chosen with the LCP control `video_codec`
/// (applies on the next Go Live).
enum VideoCodec: String, CaseIterable {
    case h264
    case hevc

    static let defaultCodec = VideoCodec.h264

    var label: String {
        switch self {
        case .h264: "H.264"
        case .hevc: "HEVC"
        }
    }
}

struct VideoEncoderSettings {
    var codec: VideoCodec = .h264
    var width: Int32 = 1920
    var height: Int32 = 1080
    var frameRate: Int = 30
    var bitrate: Int = 6_000_000
    var keyframeIntervalSeconds: Double = 1
    /// VideoToolbox low-latency rate control, H.264 only: the encoder emits
    /// each frame before the next one arrives. Requires Constrained High.
    /// HEVC has it on iOS 26.6 too, but measured on an iPhone 16 it encodes
    /// frames one at a time (25 ms at 2160p, 8 ms at 1080p), so 2160p60 and
    /// 1080p240 lose frames. The default HEVC rate control with
    /// `MaxFrameDelayCount` 0 still emits every frame before the next one
    /// and keeps up (17 ms at 2160p60, 6 ms at 1080p240). See docs/LATENCY.md.
    var lowLatency = true
}

/// Hardware H.264 or HEVC encoder (VTCompressionSession), real-time, no
/// B-frames.
/// `encode` is called on the capture queue; `onOutput` runs on a
/// VideoToolbox thread.
final class VideoEncoder: @unchecked Sendable {
    /// Called with a new format description whenever it changes (first
    /// frame, or after a reset).
    var onFormat: ((CMFormatDescription) -> Void)?
    /// Called with every encoded frame, the capture PTS (host clock) of the
    /// camera frame it was encoded from, and when the frame went into the
    /// encoder and came out (host ns).
    var onOutput: ((CMSampleBuffer, CMTime, _ encodeInNs: Int64, _ encodeOutNs: Int64) -> Void)?

    private let settings: VideoEncoderSettings
    /// Whether the running session has low-latency rate control.
    private(set) var lowLatency = false
    private var session: VTCompressionSession?
    private var lastFormat: CMFormatDescription?
    private let lock = NSLock()
    /// Capture queue only.
    private var loggedSizeMismatch = false

    init(settings: VideoEncoderSettings) {
        self.settings = settings
    }

    deinit {
        stop()
    }

    func start() throws {
        let codecType: CMVideoCodecType = settings.codec == .h264 ? kCMVideoCodecType_H264 : kCMVideoCodecType_HEVC
        var lowLatency = settings.lowLatency && settings.codec == .h264
        let session: VTCompressionSession
        do {
            session = try Self.createSession(settings, codecType: codecType, lowLatency: lowLatency)
        } catch where lowLatency {
            // Not every codec, size and OS has low-latency rate control.
            log.info("No low-latency \(self.settings.codec.rawValue) encoder (\(error.localizedDescription)), using the default rate control")
            lowLatency = false
            session = try Self.createSession(settings, codecType: codecType, lowLatency: false)
        }
        // Low-latency rate control supports Constrained Baseline and
        // Constrained High (High without B-frames, CABAC allowed).
        let profile = settings.codec == .h264
            ? (lowLatency ? kVTProfileLevel_H264_ConstrainedHigh_AutoLevel : kVTProfileLevel_H264_High_AutoLevel)
            : kVTProfileLevel_HEVC_Main_AutoLevel
        let bytesPerSecondLimit = settings.bitrate * 3 / 2 / 8
        let properties: [CFString: Any] = [
            kVTCompressionPropertyKey_RealTime: true,
            kVTCompressionPropertyKey_ProfileLevel: profile,
            kVTCompressionPropertyKey_AverageBitRate: settings.bitrate,
            kVTCompressionPropertyKey_DataRateLimits: [bytesPerSecondLimit, 1] as CFArray,
            kVTCompressionPropertyKey_ExpectedFrameRate: settings.frameRate,
            kVTCompressionPropertyKey_MaxKeyFrameInterval: Int(Double(settings.frameRate) * settings.keyframeIntervalSeconds),
            kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration: settings.keyframeIntervalSeconds,
            kVTCompressionPropertyKey_AllowFrameReordering: false,
            kVTCompressionPropertyKey_H264EntropyMode: kVTH264EntropyMode_CABAC,
            // Emit every frame before the next one is submitted.
            kVTCompressionPropertyKey_MaxFrameDelayCount: 0,
            kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality: true,
        ]
        for (key, value) in properties {
            if settings.codec != .h264, key == kVTCompressionPropertyKey_H264EntropyMode {
                continue
            }
            let status = VTSessionSetProperty(session, key: key, value: value as CFTypeRef)
            if status != noErr {
                log.error("Failed to set \(key as String): \(status)")
            }
        }
        VTCompressionSessionPrepareToEncodeFrames(session)
        self.lowLatency = lowLatency
        lock.withLock {
            self.session = session
            lastFormat = nil
        }
        log.info("Started \(self.settings.codec.rawValue) \(self.settings.width)x\(self.settings.height)@\(self.settings.frameRate) \(self.settings.bitrate) bit/s, low latency \(lowLatency)")
    }

    private static func createSession(_ settings: VideoEncoderSettings, codecType: CMVideoCodecType,
                                      lowLatency: Bool) throws -> VTCompressionSession
    {
        let specification: CFDictionary? = lowLatency
            ? [kVTVideoEncoderSpecification_EnableLowLatencyRateControl: true] as CFDictionary
            : nil
        var session: VTCompressionSession?
        let status = VTCompressionSessionCreate(
            allocator: kCFAllocatorDefault,
            width: settings.width,
            height: settings.height,
            codecType: codecType,
            encoderSpecification: specification,
            imageBufferAttributes: nil,
            compressedDataAllocator: nil,
            outputCallback: nil,
            refcon: nil,
            compressionSessionOut: &session
        )
        guard status == noErr, let session else {
            throw VideoEncoderError.createFailed(status)
        }
        return session
    }

    func stop() {
        let session = lock.withLock {
            let session = self.session
            self.session = nil
            return session
        }
        guard let session else {
            return
        }
        VTCompressionSessionCompleteFrames(session, untilPresentationTimeStamp: .invalid)
        VTCompressionSessionInvalidate(session)
    }

    /// Changes the target bitrate of the running session.
    func setBitrate(_ bitrate: Int) {
        guard let session = lock.withLock({ self.session }) else {
            return
        }
        let bytesPerSecondLimit = bitrate * 3 / 2 / 8
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_AverageBitRate, value: bitrate as CFNumber)
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_DataRateLimits,
                             value: [bytesPerSecondLimit, 1] as CFArray)
        log.info("Bitrate \(bitrate) bit/s")
    }

    /// Tells the running session the capture frame rate changed.
    func setFrameRate(_ frameRate: Int) {
        guard let session = lock.withLock({ self.session }) else {
            return
        }
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_ExpectedFrameRate, value: frameRate as CFNumber)
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_MaxKeyFrameInterval,
                             value: Int(Double(frameRate) * settings.keyframeIntervalSeconds) as CFNumber)
    }

    func encode(_ sampleBuffer: CMSampleBuffer) {
        guard let session = lock.withLock({ self.session }),
              let imageBuffer = sampleBuffer.imageBuffer
        else {
            return
        }
        // A frame from before an orientation change: never let the session
        // scale it into the stream's frame size.
        guard CVPixelBufferGetWidth(imageBuffer) == Int(settings.width),
              CVPixelBufferGetHeight(imageBuffer) == Int(settings.height)
        else {
            if !loggedSizeMismatch {
                loggedSizeMismatch = true
                log.info("Dropping \(CVPixelBufferGetWidth(imageBuffer))x\(CVPixelBufferGetHeight(imageBuffer)) frames, encoding \(self.settings.width)x\(self.settings.height)")
            }
            return
        }
        let capturePts = sampleBuffer.presentationTimeStamp
        let encodeInNs = HostClock.nowNs()
        let status = VTCompressionSessionEncodeFrame(
            session,
            imageBuffer: imageBuffer,
            presentationTimeStamp: capturePts,
            duration: sampleBuffer.duration,
            frameProperties: nil,
            infoFlagsOut: nil
        ) { [weak self] status, _, encoded in
            guard status == noErr, let encoded, let self else {
                if status != noErr {
                    log.error("Encode failed: \(status)")
                }
                return
            }
            self.handleOutput(encoded, capturePts: capturePts, encodeInNs: encodeInNs, encodeOutNs: HostClock.nowNs())
        }
        if status != noErr {
            log.error("EncodeFrame failed: \(status)")
        }
    }

    private func handleOutput(_ sampleBuffer: CMSampleBuffer, capturePts: CMTime, encodeInNs: Int64, encodeOutNs: Int64) {
        if let format = sampleBuffer.formatDescription {
            let changed = lock.withLock {
                defer { lastFormat = format }
                return lastFormat.map { !CFEqual($0, format) } ?? true
            }
            if changed {
                onFormat?(format)
            }
        }
        onOutput?(sampleBuffer, capturePts, encodeInNs, encodeOutNs)
    }
}

enum VideoEncoderError: LocalizedError {
    case createFailed(OSStatus)

    var errorDescription: String? {
        switch self {
        case let .createFailed(status): "Could not create the video encoder (\(status))"
        }
    }
}
