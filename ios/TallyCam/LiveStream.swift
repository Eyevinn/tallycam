import CoreMedia
import Foundation
import os
#if canImport(TallyCamCoreFFI)
    import TallyCamCoreFFI
#endif

private let log = Logger(subsystem: "se.eyevinn.tallycam", category: "live")

/// One live session: camera sample buffers and microphone PCM -> encoders
/// -> MPEG-TS -> SRT. The muxer and the SRT connection live on `queue`; the
/// video encoder runs on the capture queue and VideoToolbox threads, the
/// audio encoder on the microphone's delivery thread, and both hop onto
/// `queue` with output.
final class LiveStream: @unchecked Sendable {
    enum Event {
        /// `video`: what the encoder runs, for the log (codec, size, rate
        /// control).
        case connected(video: String)
        /// The connection was lost; the SRT caller connects again while the
        /// encoders keep running.
        case reconnecting(reason: String)
        case stopped(reason: String?)
    }

    struct Stats {
        var bytesSent: UInt64
        var srt: SrtStats?
        /// Per-stage latency since the last call.
        var latency: LatencyReport
        /// Samples muxed after their PCR deadline, since the start.
        var lateVideoFrames: Int
        var lateAudioFrames: Int
    }

    private let queue = DispatchQueue(label: "live.stream", qos: .userInitiated)
    private let url: SrtUrl
    // Owned by `queue` after init.
    private var videoSettings: VideoEncoderSettings
    /// Nil without a microphone.
    private let mic: MicCapture?
    private let audioCodec: AudioCodec
    private let outputs: CaptureOutputs
    private let onEvent: @Sendable (Event) -> Void
    // Owned by `queue`.
    private var connection: SrtConnection?
    /// The core's MPEG-TS muxer; it queues onto the SRT caller itself.
    private var muxer: CoreTsMuxer?
    private var videoEncoder: VideoEncoder?
    private var stopped = false
    /// LCP SEI frame counter, one per encoded frame; wraps.
    private var frameCounter: UInt32 = 0
    private var meter = LatencyMeter()
    private var audioBufferFrames = 0
    private var loggedStart = false

    init(url: SrtUrl,
         videoSettings: VideoEncoderSettings,
         mic: MicCapture?,
         audioCodec: AudioCodec,
         outputs: CaptureOutputs,
         onEvent: @escaping @Sendable (Event) -> Void)
    {
        self.url = url
        self.videoSettings = videoSettings
        self.mic = mic
        self.audioCodec = audioCodec
        self.outputs = outputs
        self.onEvent = onEvent
    }

    func start() {
        queue.async { [self] in
            // Reconnect: a listener restart or a network change while live
            // comes back by itself. A failure before the first connection
            // still ends the session with its reason (SrtConnection).
            var options = url.options
            options.reconnect = true
            let connection = SrtConnection(options: options, queue: queue)
            connection.onEvent = { [weak self] event in
                guard let self else {
                    return
                }
                switch event {
                case .connected:
                    if muxer == nil {
                        startMedia()
                    } else {
                        // Reconnected: the listener gets PAT/PMT with the next
                        // keyframe (at most a second away).
                        onEvent(.connected(video: videoDescription))
                    }
                case let .disconnected(reason, reconnecting: true):
                    log.info("Connection lost, reconnecting: \(reason)")
                    onEvent(.reconnecting(reason: reason))
                case let .disconnected(reason, reconnecting: false):
                    stopOnQueue(reason: reason)
                }
            }
            self.connection = connection
            connection.open()
        }
    }

    /// `reason` nil means stopped by the user or the hub.
    func stop(reason: String? = nil) {
        queue.async { [self] in
            stopOnQueue(reason: reason)
        }
    }

    func setBitrate(_ bitrate: Int) {
        queue.async { [self] in
            videoSettings.bitrate = bitrate
            videoEncoder?.setBitrate(bitrate)
        }
    }

    func setFrameRate(_ frameRate: Int) {
        queue.async { [self] in
            videoSettings.frameRate = frameRate
            videoEncoder?.setFrameRate(frameRate)
        }
    }

    func stats(_ completion: @escaping @Sendable (Stats) -> Void) {
        queue.async { [self] in
            let srt = connection?.stats
            let mux = muxer?.stats
            completion(Stats(bytesSent: srt?.bytesSent ?? 0, srt: srt,
                             latency: meter.report(),
                             lateVideoFrames: mux?.lateVideoFrames ?? 0,
                             lateAudioFrames: mux?.lateAudioFrames ?? 0))
        }
    }

    private func startMedia() {
        let codec = mic == nil ? nil : audioCodec
        // The PCR delay follows the audio codec (the core's default).
        guard let handle = connection?.hotHandle, handle != 0,
              let muxer = CoreTsMuxer(srtHandle: handle, videoCodec: videoSettings.codec, audioCodec: codec)
        else {
            stopOnQueue(reason: "Could not start the MPEG-TS muxer")
            return
        }
        self.muxer = muxer
        connection?.onFrameSent = { [weak self] captureNs, enqueuedNs, sentNs, last in
            guard let self else {
                return
            }
            if last {
                meter.record(.sendFrame, ns: sentNs - enqueuedNs)
            } else {
                meter.record(.sendQueue, ns: sentNs - enqueuedNs)
                meter.record(.captureToSend, ns: sentNs - captureNs)
            }
        }

        let videoEncoder = VideoEncoder(settings: videoSettings)
        videoEncoder.onFormat = { [weak self] format in
            self?.queue.async { self?.setVideoFormat(format) }
        }
        videoEncoder.onOutput = { [weak self] sampleBuffer, capturePts, encodeInNs, encodeOutNs in
            self?.queue.async {
                self?.writeVideo(sampleBuffer, capturePts: capturePts, encodeInNs: encodeInNs, encodeOutNs: encodeOutNs)
            }
        }
        do {
            try videoEncoder.start()
        } catch {
            stopOnQueue(reason: error.localizedDescription)
            return
        }
        self.videoEncoder = videoEncoder

        if let mic {
            // Used only on the microphone's delivery thread.
            let audioEncoder = AudioEncoder(codec: audioCodec)
            audioEncoder.onFormat = { [weak self] sampleRate, channels in
                self?.queue.async { self?.setAudioFormat(sampleRate: sampleRate, channels: channels) }
            }
            audioEncoder.onOutput = { [weak self] packet, ptsNs, encodeNs in
                self?.queue.async { self?.writeAudio(packet, ptsNs: ptsNs, encodeNs: encodeNs) }
            }
            mic.setSink { [weak self] samples, captureNs, arrivedNs, sampleRate in
                let frames = samples.count
                self?.queue.async { self?.audioInput(captureNs: captureNs, frames: frames, arrivedNs: arrivedNs) }
                audioEncoder.encode(samples, captureNs: captureNs, sampleRate: sampleRate)
            }
        }
        outputs.setVideoSink { videoEncoder.encode($0) }
        log.info("Live, video \(self.videoSettings.codec.label) \(self.videoSettings.width)x\(self.videoSettings.height)@\(self.videoSettings.frameRate), audio \(self.mic == nil ? "none" : self.audioCodec.label)")
        onEvent(.connected(video: videoDescription))
    }

    /// What the video encoder runs, for the log.
    private var videoDescription: String {
        "\(videoSettings.codec.label) \(videoSettings.width)x\(videoSettings.height)"
            + " at \(videoSettings.frameRate) fps, \(videoSettings.bitrate / 1000) kbit/s"
            + (videoEncoder?.lowLatency == true ? ", low-latency rate control" : "")
    }

    /// Host ns of a capture timestamp (host time clock).
    private static func hostNs(_ time: CMTime) -> Int64 {
        HostClock.ns(fromTicks: CMClockConvertHostTimeToSystemUnits(time))
    }

    private func setVideoFormat(_ format: CMFormatDescription) {
        if let muxer, !muxer.setVideoFormat(format) {
            log.error("No parameter sets in the \(self.videoSettings.codec.label) format description")
        }
    }

    private func setAudioFormat(sampleRate: Double, channels: Int) {
        if let muxer, !muxer.setAudioFormat(sampleRate: sampleRate, channels: channels) {
            log.error("\(self.audioCodec.label) at \(sampleRate) Hz, \(channels) ch cannot be muxed")
        }
    }

    private func audioInput(captureNs: Int64, frames: Int, arrivedNs: Int64) {
        meter.record(.audioCaptureToEncode, ns: arrivedNs - captureNs)
        if frames != audioBufferFrames {
            audioBufferFrames = frames
            log.info("Microphone buffers of \(frames) frames")
        }
    }

    /// `ptsNs`: the capture time of the frame's first sample minus the
    /// encoder delay (see `AudioTiming.ptsNs`).
    private func writeAudio(_ packet: Data, ptsNs: Int64, encodeNs: Int64) {
        let nowNs = HostClock.nowNs()
        meter.record(.audioEncode, ns: encodeNs)
        meter.record(.audioCaptureToMux, ns: nowNs - ptsNs)
        if let muxer {
            connection?.noteSendResult(muxer.writeAudio(packet, ptsNs: ptsNs, nowNs: nowNs))
        }
    }

    /// Stamps the frame with its capture time: TAI from the phone's own
    /// disciplined clock (lock free), uncertainty = its maximum error.
    private func writeVideo(_ sampleBuffer: CMSampleBuffer, capturePts: CMTime, encodeInNs: Int64, encodeOutNs: Int64) {
        guard let muxer else {
            return
        }
        let hostNs = Self.hostNs(capturePts)
        meter.record(.captureToEncode, ns: encodeInNs - hostNs)
        meter.record(.encode, ns: encodeOutNs - encodeInNs)
        // The stamp on the reference model's timescale: SEI and PTS.
        let exposureNs = CaptureCalibration.exposureNs(stampNs: hostNs)
        let mapping = ClockMapping.at(hostNs: exposureNs)
        let uncertaintyUs = mapping.synced
            ? TimestampSei.uncertaintyUs(fromNs: mapping.maxErrorNs)
            : TimestampSei.uncertaintyUnknown
        let counter = frameCounter
        frameCounter &+= 1
        // The core builds the SEI and puts it before the first slice.
        let flags = UInt8((mapping.synced ? LC_SEI_FLAG_SYNCED : 0) | LC_SEI_FLAG_TAI)
        let result = muxer.writeVideo(sampleBuffer, ptsNs: exposureNs, sendCaptureNs: hostNs,
                                      sei: (UInt64(clamping: mapping.taiNs), flags, counter, uncertaintyUs),
                                      nowNs: HostClock.nowNs())
        guard result != LC_TS_SKIPPED else {
            return
        }
        // Muxing and queueing onto the SRT caller.
        meter.record(.mux, ns: HostClock.nowNs() - encodeOutNs)
        if !loggedStart, let startNs = muxer.stats.startNs {
            loggedStart = true
            log.info("Stream starts at host time \(startNs)")
        }
        connection?.noteSendResult(result)
    }

    private func stopOnQueue(reason: String?) {
        guard !stopped else {
            return
        }
        stopped = true
        outputs.setVideoSink(nil)
        mic?.setSink(nil)
        videoEncoder?.stop()
        videoEncoder = nil
        muxer = nil
        connection?.close()
        connection = nil
        log.info("Stopped: \(reason ?? "by user")")
        onEvent(.stopped(reason: reason))
    }
}
