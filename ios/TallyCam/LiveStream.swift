import CoreMedia
import Foundation
import os
import os.lock
#if canImport(TallyCamCoreFFI)
    import TallyCamCoreFFI
#endif

private let log = Logger(subsystem: "se.eyevinn.tallycam", category: "live")

/// One live session: camera sample buffers and microphone PCM -> encoders
/// -> MPEG-TS -> SRT. The SRT connection and the session's lifecycle live on
/// `queue`. The video encoder runs on the capture queue and VideoToolbox
/// threads, the audio encoder on the microphone's delivery thread; each
/// muxes its own output on its own thread (`MuxPath`, under a lock) instead
/// of hopping onto `queue`, which saves a thread switch per frame.
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

    private let queue = DispatchQueue(label: "live.stream", qos: .userInteractive)
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
    private var videoEncoder: VideoEncoder?
    private var stopped = false
    /// The muxer and the latency meter; any thread.
    private let mux = MuxPath()

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
            let mux = mux
            let connection = SrtConnection(options: options, queue: queue) { captureNs, enqueuedNs, sentNs, last in
                mux.frameSent(captureNs: captureNs, enqueuedNs: enqueuedNs, sentNs: sentNs, last: last)
            }
            connection.onEvent = { [weak self] event in
                guard let self else {
                    return
                }
                switch event {
                case .connected:
                    if videoEncoder == nil {
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
            let (latency, muxStats) = mux.report()
            completion(Stats(bytesSent: srt?.bytesSent ?? 0, srt: srt,
                             latency: latency,
                             lateVideoFrames: muxStats?.lateVideoFrames ?? 0,
                             lateAudioFrames: muxStats?.lateAudioFrames ?? 0))
        }
    }

    private func startMedia() {
        let codec = mic == nil ? nil : audioCodec
        // The PCR delay follows the audio codec (the core's default).
        guard let handle = connection?.hotHandle, handle != 0,
              let muxer = CoreTsMuxer(srtHandle: handle, videoCodec: videoSettings.codec, audioCodec: codec,
                                      pcrDelayNs: PcrDelay.ns(video: videoSettings, audio: codec))
        else {
            stopOnQueue(reason: "Could not start the MPEG-TS muxer")
            return
        }
        mux.start(muxer, audioCodec: audioCodec)

        // VideoToolbox and microphone threads mux their own output.
        let mux = mux
        let videoEncoder = VideoEncoder(settings: videoSettings)
        mux.onWantsKeyframe = { [weak videoEncoder] in
            videoEncoder?.requestKeyframe()
        }
        videoEncoder.onFormat = { format in
            mux.setVideoFormat(format)
        }
        videoEncoder.onOutput = { sampleBuffer, capturePts, encodeInNs, encodeOutNs in
            mux.writeVideo(sampleBuffer, capturePts: capturePts, encodeInNs: encodeInNs, encodeOutNs: encodeOutNs)
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
            audioEncoder.onFormat = { sampleRate, channels in
                mux.setAudioFormat(sampleRate: sampleRate, channels: channels)
            }
            audioEncoder.onOutput = { packet, ptsNs, encodeNs in
                mux.writeAudio(packet, ptsNs: ptsNs, encodeNs: encodeNs)
            }
            mic.setSink { samples, captureNs, arrivedNs, sampleRate in
                mux.audioInput(captureNs: captureNs, frames: samples.count, arrivedNs: arrivedNs)
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

    private func stopOnQueue(reason: String?) {
        guard !stopped else {
            return
        }
        stopped = true
        outputs.setVideoSink(nil)
        mic?.setSink(nil)
        videoEncoder?.stop()
        videoEncoder = nil
        mux.stop()
        connection?.close()
        connection = nil
        log.info("Stopped: \(reason ?? "by user")")
        onEvent(.stopped(reason: reason))
    }
}

/// The muxer and the latency meter, shared by the threads that produce
/// media (VideoToolbox output, microphone delivery) and the SRT thread
/// (frame-sent reports). Each call takes the lock briefly: muxing a frame
/// is tens of microseconds, and the core queues the packets onto the SRT
/// caller without blocking.
private final class MuxPath: @unchecked Sendable {
    private let lock = OSAllocatedUnfairLock()
    // Guarded by `lock`.
    private var muxer: CoreTsMuxer?
    private var audioLabel = ""
    private var meter = LatencyMeter()
    /// LCP SEI frame counter, one per encoded frame; wraps.
    private var frameCounter: UInt32 = 0
    private var loggedStart = false
    private var audioBufferFrames = 0
    private var queueFullCount = 0
    /// Asks the encoder for a keyframe; set before the encoder starts.
    var onWantsKeyframe: (@Sendable () -> Void)?

    func start(_ muxer: CoreTsMuxer, audioCodec: AudioCodec) {
        lock.withLockUnchecked {
            self.muxer = muxer
            audioLabel = audioCodec.label
        }
    }

    func stop() {
        lock.withLockUnchecked {
            muxer = nil
        }
    }

    /// p50/p95/max per stage since the last call, and the muxer's counters.
    func report() -> (LatencyReport, CoreTsMuxer.Stats?) {
        lock.withLockUnchecked { (meter.report(), muxer?.stats) }
    }

    func setVideoFormat(_ format: CMFormatDescription) {
        let ok = lock.withLockUnchecked { muxer.map { $0.setVideoFormat(format) } ?? true }
        if !ok {
            log.error("No parameter sets in the video format description")
        }
    }

    func setAudioFormat(sampleRate: Double, channels: Int) {
        let (ok, label) = lock.withLockUnchecked {
            (muxer.map { $0.setAudioFormat(sampleRate: sampleRate, channels: channels) } ?? true, audioLabel)
        }
        if !ok {
            log.error("\(label) at \(sampleRate) Hz, \(channels) ch cannot be muxed")
        }
    }

    /// On the SRT thread: the first (`last` false) or last packet of a
    /// video frame went out.
    func frameSent(captureNs: Int64, enqueuedNs: Int64, sentNs: Int64, last: Bool) {
        lock.withLockUnchecked {
            if last {
                meter.record(.sendFrame, ns: sentNs - enqueuedNs)
            } else {
                meter.record(.sendQueue, ns: sentNs - enqueuedNs)
                meter.record(.captureToSend, ns: sentNs - captureNs)
            }
        }
    }

    /// On the microphone's delivery thread, per buffer.
    func audioInput(captureNs: Int64, frames: Int, arrivedNs: Int64) {
        let changed = lock.withLockUnchecked {
            meter.record(.audioCaptureToEncode, ns: arrivedNs - captureNs)
            defer { audioBufferFrames = frames }
            return frames != audioBufferFrames
        }
        if changed {
            log.info("Microphone buffers of \(frames) frames")
        }
    }

    /// On the microphone's delivery thread. `ptsNs`: the capture time of
    /// the frame's first sample minus the encoder delay (see
    /// `AudioTiming.ptsNs`).
    func writeAudio(_ packet: UnsafeRawBufferPointer, ptsNs: Int64, encodeNs: Int64) {
        let result: CoreTsMuxer.Result? = lock.withLockUnchecked {
            guard let muxer else {
                return nil
            }
            let nowNs = HostClock.nowNs()
            meter.record(.audioEncode, ns: encodeNs)
            meter.record(.audioCaptureToMux, ns: nowNs - ptsNs)
            return muxer.writeAudio(packet, ptsNs: ptsNs, nowNs: nowNs)
        }
        if let result {
            noteSendResult(result)
        }
    }

    /// On a VideoToolbox thread. Stamps the frame with its capture time:
    /// TAI from the phone's own disciplined clock (lock free), uncertainty =
    /// its maximum error.
    func writeVideo(_ sampleBuffer: CMSampleBuffer, capturePts: CMTime, encodeInNs: Int64, encodeOutNs: Int64) {
        let hostNs = HostClock.ns(fromTicks: CMClockConvertHostTimeToSystemUnits(capturePts))
        // The stamp on the reference model's timescale: SEI and PTS.
        let exposureNs = CaptureCalibration.exposureNs(stampNs: hostNs)
        let mapping = ClockMapping.at(hostNs: exposureNs)
        let uncertaintyUs = mapping.synced
            ? TimestampSei.uncertaintyUs(fromNs: mapping.maxErrorNs)
            : TimestampSei.uncertaintyUnknown
        // The core builds the SEI and puts it before the first slice.
        let flags = UInt8((mapping.synced ? LC_SEI_FLAG_SYNCED : 0) | LC_SEI_FLAG_TAI)
        var wantsKeyframe = false
        let (result, startNs) = lock.withLockUnchecked { () -> (CoreTsMuxer.Result, Int64?) in
            guard let muxer else {
                return (CoreTsMuxer.Result(LC_TS_SKIPPED), nil)
            }
            meter.record(.captureToEncode, ns: encodeInNs - hostNs)
            meter.record(.encode, ns: encodeOutNs - encodeInNs)
            let counter = frameCounter
            frameCounter &+= 1
            let result = muxer.writeVideo(sampleBuffer, ptsNs: exposureNs, sendCaptureNs: hostNs,
                                          sei: (UInt64(clamping: mapping.taiNs), flags, counter, uncertaintyUs),
                                          nowNs: HostClock.nowNs())
            guard result != LC_TS_SKIPPED else {
                wantsKeyframe = muxer.wantsKeyframe
                return (result, nil)
            }
            // Muxing and queueing onto the SRT caller.
            meter.record(.mux, ns: HostClock.nowNs() - encodeOutNs)
            guard !loggedStart, let startNs = muxer.stats.startNs else {
                return (result, nil)
            }
            loggedStart = true
            return (result, startNs)
        }
        if wantsKeyframe {
            onWantsKeyframe?()
        }
        if let startNs {
            log.info("Stream starts at host time \(startNs)")
        }
        if result != LC_TS_SKIPPED {
            noteSendResult(result)
        }
    }

    /// Logs runs of a full SRT send queue (packets are dropped while not
    /// connected, and counted in the SRT stats).
    private func noteSendResult(_ result: Int32) {
        let event: (full: Bool, count: Int)? = lock.withLockUnchecked {
            if result == LC_SRT_QUEUE_FULL {
                queueFullCount += 1
                return queueFullCount == 1 ? (true, 0) : nil
            } else if result > 0, queueFullCount > 0 {
                defer { queueFullCount = 0 }
                return (false, queueFullCount)
            }
            return nil
        }
        switch event {
        case (true, _)?:
            log.warning("SRT send queue full, dropping")
        case let (false, count)?:
            log.info("SRT send queue drained after \(count) dropped sends")
        case nil:
            break
        }
    }
}
