import Foundation

/// UI-facing state for going live: URL, state machine and stats.
@MainActor
final class StreamController: ObservableObject {
    enum State: Equatable {
        case idle
        case connecting
        case live
    }

    struct Stats {
        var bitrateKbps: Int
        var rttMs: Double?
        var retransmits: Int?
        var dropped: Int?
        /// The SRT latency the listener agreed to, ms.
        var srtLatencyMs: Int?
        var latency: LatencyReport?
        var lateVideoFrames: Int?
        var lateAudioFrames: Int?
    }

    private static let urlKey = "srtUrl"
    /// Set only when the operator chose the bitrate; otherwise it follows
    /// the format and codec (`VideoBitrate.defaultKbps`).
    private static let bitrateKey = "videoBitrateKbps"
    private static let audioCodecKey = "audioCodec"
    private static let videoCodecKey = "videoCodec"
    /// Launch argument `-videoLowLatency NO` turns VideoToolbox low-latency
    /// rate control off (for latency comparisons).
    private static let lowLatencyKey = "videoLowLatency"

    @Published var url: String {
        didSet { UserDefaults.standard.set(url, forKey: Self.urlKey) }
    }

    /// The name set by the operator (`camera_name`) or in Settings; empty
    /// for the default (`CameraName`).
    @Published var cameraName: String {
        didSet {
            if let name = CameraName.normalized(cameraName) {
                UserDefaults.standard.set(name, forKey: CameraName.defaultsKey)
            } else {
                UserDefaults.standard.removeObject(forKey: CameraName.defaultsKey)
            }
        }
    }

    /// The camera's display name, as `hello.device.name` carries it.
    var displayName: String {
        CameraName.display(custom: cameraName, model: CaptureCalibration.model,
                           deviceId: DeviceKey.current()?.deviceId)
    }

    @Published private(set) var state: State = .idle
    @Published private(set) var message: String?
    @Published private(set) var stats: Stats?
    /// When the stream went live (connected), for the uptime display.
    @Published private(set) var liveSince: Date?
    /// The bitrate the operator set; nil follows the format and codec.
    @Published private(set) var manualBitrateKbps: Int? {
        didSet {
            if let manualBitrateKbps {
                UserDefaults.standard.set(manualBitrateKbps, forKey: Self.bitrateKey)
            } else {
                UserDefaults.standard.removeObject(forKey: Self.bitrateKey)
            }
        }
    }

    /// Applies on the next Go Live.
    @Published var audioCodec: AudioCodec {
        didSet { UserDefaults.standard.set(audioCodec.rawValue, forKey: Self.audioCodecKey) }
    }

    /// Applies on the next Go Live. Changing it returns the bitrate to the
    /// codec's default.
    @Published var videoCodec: VideoCodec {
        didSet {
            UserDefaults.standard.set(videoCodec.rawValue, forKey: Self.videoCodecKey)
            if videoCodec != oldValue {
                bitrateFollowsFormat()
            }
        }
    }

    /// The capture format, so the encoder and the bitrate follow it.
    @Published private(set) var captureFormat = CaptureFormat.initial
    /// False until the camera reported its first format.
    private var captureFormatKnown = false
    /// What the running stream's video encoder does, for the log.
    private(set) var liveVideo: String?
    /// The codec of the running stream (nil when idle).
    private var liveVideoCodec: VideoCodec?
    /// The bitrate the running encoder was last given, kbit/s.
    private var appliedBitrateKbps: Int?

    private var stream: LiveStream?
    private var streamId: UUID?
    /// Holds the stream orientation lock while a stream runs.
    private weak var liveCapture: Capture?
    private var statsTimer: Timer?
    /// Set while in the background after stopping a live stream.
    private var resumeCapture: Capture?
    /// Set when the app came back before the stream finished stopping.
    private var restartCapture: Capture?
    private var lastBytesSent: UInt64 = 0
    private var lastStatsTime = Date()

    init() {
        url = UserDefaults.standard.string(forKey: Self.urlKey) ?? ""
        cameraName = CameraName.stored ?? ""
        let storedBitrate = UserDefaults.standard.integer(forKey: Self.bitrateKey)
        manualBitrateKbps = storedBitrate == 0 ? nil : storedBitrate
        audioCodec = UserDefaults.standard.string(forKey: Self.audioCodecKey)
            .flatMap(AudioCodec.init(rawValue:)) ?? .defaultCodec
        videoCodec = UserDefaults.standard.string(forKey: Self.videoCodecKey)
            .flatMap(VideoCodec.init(rawValue:)) ?? .defaultCodec
    }

    /// The codec the bitrate is for: the running stream's while live.
    private var bitrateCodec: VideoCodec {
        liveVideoCodec ?? videoCodec
    }

    /// The `bitrate_kbps` range for the current format and codec.
    var bitrateRange: ClosedRange<Int> {
        let format = captureFormat
        return VideoBitrate.minKbps ... VideoBitrate.maxKbps(width: format.width, height: format.height,
                                                              frameRate: format.frameRate, codec: bitrateCodec)
    }

    /// The video bitrate: the operator's, within `bitrateRange`, or the
    /// default for the format and codec.
    var bitrateKbps: Int {
        let range = bitrateRange
        if let manualBitrateKbps {
            return min(max(manualBitrateKbps, range.lowerBound), range.upperBound)
        }
        let format = captureFormat
        return VideoBitrate.defaultKbps(width: format.width, height: format.height,
                                        frameRate: format.frameRate, codec: bitrateCodec)
    }

    /// Whether the bitrate follows the format and codec.
    var bitrateIsAuto: Bool {
        manualBitrateKbps == nil
    }

    /// Returns the value applied, clamped to `bitrateRange`. Applied live;
    /// kept until the format or codec changes.
    @discardableResult
    func setBitrate(kbps: Int) -> Int {
        let clamped = min(max(kbps, bitrateRange.lowerBound), bitrateRange.upperBound)
        manualBitrateKbps = clamped
        applyBitrateLive()
        return clamped
    }

    /// A new capture format (resolution, frame rate): the encoder follows
    /// the frame rate live, the bitrate returns to the format's default.
    func setCaptureFormat(_ format: CaptureFormat) {
        let first = !captureFormatKnown
        captureFormatKnown = true
        guard format != captureFormat else {
            return
        }
        let frameRateChanged = format.frameRate != captureFormat.frameRate
        captureFormat = format
        // The format reported at launch keeps a bitrate saved earlier.
        if !first {
            bitrateFollowsFormat()
        }
        if frameRateChanged {
            stream?.setFrameRate(format.frameRate)
        }
        applyBitrateLive()
    }

    private func bitrateFollowsFormat() {
        if manualBitrateKbps != nil {
            manualBitrateKbps = nil
        }
        applyBitrateLive()
    }

    private func applyBitrateLive() {
        guard let stream, bitrateKbps != appliedBitrateKbps else {
            return
        }
        appliedBitrateKbps = bitrateKbps
        stream.setBitrate(bitrateKbps * 1000)
    }

    func goLive(capture: Capture) {
        guard state == .idle else {
            return
        }
        guard !url.trimmingCharacters(in: .whitespaces).isEmpty else {
            message = "No stream URL, scan the QR code in the control hub"
            return
        }
        let srtUrl: SrtUrl
        do {
            srtUrl = try SrtUrl(string: url)
        } catch {
            message = error.localizedDescription
            return
        }
        message = nil
        stats = nil
        state = .connecting
        let id = UUID()
        // The frame size is fixed for the whole stream.
        capture.lockStreamOrientation()
        let format = capture.lockFormat()
        liveCapture = capture
        setCaptureFormat(format)
        liveVideoCodec = videoCodec
        let size = capture.streamSize
        let lowLatency = UserDefaults.standard.object(forKey: Self.lowLatencyKey) == nil
            || UserDefaults.standard.bool(forKey: Self.lowLatencyKey)
        appliedBitrateKbps = bitrateKbps
        let stream = LiveStream(url: srtUrl,
                                videoSettings: VideoEncoderSettings(codec: videoCodec,
                                                                    width: size.width, height: size.height,
                                                                    frameRate: format.frameRate,
                                                                    bitrate: bitrateKbps * 1000,
                                                                    lowLatency: lowLatency),
                                mic: capture.hasAudio ? capture.mic : nil,
                                audioCodec: audioCodec,
                                outputs: capture.outputs)
        { [weak self] event in
            Task { @MainActor in
                self?.handle(event: event, streamId: id)
            }
        }
        self.stream = stream
        streamId = id
        stream.start()
    }

    func stop() {
        resumeCapture = nil
        restartCapture = nil
        stream?.stop()
    }

    /// iOS stops the camera in the background, so a live stream stops here
    /// with a reason the hub sees, and comes back in `enteredForeground`.
    func enteredBackground(capture: Capture) {
        guard state != .idle else {
            return
        }
        resumeCapture = capture
        restartCapture = nil
        stream?.stop(reason: "App went to the background (iOS stops the camera)")
    }

    /// Goes live again if the stream was live when the app left.
    func enteredForeground() {
        guard let capture = resumeCapture else {
            return
        }
        resumeCapture = nil
        if state == .idle {
            goLive(capture: capture)
        } else {
            // Still stopping: start again once it has.
            restartCapture = capture
        }
    }

    private func handle(event: LiveStream.Event, streamId id: UUID) {
        guard id == streamId else {
            return
        }
        switch event {
        case let .connected(video):
            liveVideo = video
            message = nil
            state = .live
            liveSince = Date()
            lastBytesSent = 0
            lastStatsTime = Date()
            startStatsTimer()
        case let .reconnecting(reason):
            // Still live for the hub (`live` stays true); the status shows
            // connecting with the reason until the caller is back.
            message = reason
            state = .connecting
            liveSince = nil
            stats = nil
            statsTimer?.invalidate()
            statsTimer = nil
        case let .stopped(reason):
            stream = nil
            streamId = nil
            state = .idle
            liveSince = nil
            stats = nil
            message = reason
            liveCapture?.unlockStreamOrientation()
            liveCapture?.unlockFormat()
            liveCapture = nil
            liveVideoCodec = nil
            liveVideo = nil
            appliedBitrateKbps = nil
            statsTimer?.invalidate()
            statsTimer = nil
            if let capture = restartCapture {
                restartCapture = nil
                goLive(capture: capture)
            }
        }
    }

    private func startStatsTimer() {
        statsTimer?.invalidate()
        statsTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in
                self?.pollStats()
            }
        }
    }

    private func pollStats() {
        stream?.stats { [weak self] stats in
            Task { @MainActor in
                self?.update(stats: stats)
            }
        }
    }

    private func update(stats: LiveStream.Stats) {
        let now = Date()
        let elapsed = now.timeIntervalSince(lastStatsTime)
        guard elapsed > 0 else {
            return
        }
        let bytes = stats.bytesSent >= lastBytesSent ? stats.bytesSent - lastBytesSent : stats.bytesSent
        let kbps = Double(bytes) * 8 / elapsed / 1000
        lastBytesSent = stats.bytesSent
        lastStatsTime = now
        var numbers = Stats(bitrateKbps: Int(kbps.rounded()))
        if let srt = stats.srt {
            numbers.rttMs = (srt.rttMs * 10).rounded() / 10
            numbers.retransmits = Int(clamping: srt.retransmittedPackets)
            numbers.dropped = Int(clamping: srt.droppedPackets)
            numbers.srtLatencyMs = srt.negotiatedLatencyMs == 0 ? nil : Int(srt.negotiatedLatencyMs)
        }
        numbers.latency = stats.latency
        numbers.lateVideoFrames = stats.lateVideoFrames
        numbers.lateAudioFrames = stats.lateAudioFrames
        self.stats = numbers
    }
}
