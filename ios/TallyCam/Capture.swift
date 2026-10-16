@preconcurrency import AVFoundation
import os

private let log = Logger(subsystem: "se.eyevinn.tallycam", category: "capture")

/// Owns the capture session (back camera, 1080p30 unless the `resolution`
/// and `frame_rate` controls say otherwise) and the microphone. Frames are
/// rotated on the capture connection to the stream orientation, for example
/// 1920x1080 in landscape and 1080x1920 in portrait. The microphone is not
/// part of the capture session (`MicCapture`): AVCaptureSession hands audio
/// over about 60 ms after capture, an audio unit within a few ms.
@MainActor
final class Capture: ObservableObject {
    private static let orientationKey = "orientationMode"

    let session: AVCaptureSession
    let outputs = CaptureOutputs()
    let mic = MicCapture()
    let controls: CameraControls
    @Published var status = "Starting camera"
    /// Why there is no camera picture (no access, no camera), if so.
    @Published private(set) var cameraError: String?
    @Published private(set) var hasAudio = false
    /// Bumped when the video input changes, so the preview re-applies its rotation.
    @Published private(set) var inputGeneration = 0
    @Published private(set) var orientationMode: OrientationMode
    /// What the encoder gets and the preview shows.
    @Published private(set) var streamOrientation: StreamOrientation = .landscapeHomeRight
    /// The active capture format (landscape size, frame rate).
    @Published private(set) var format = CaptureFormat.initial

    private let sessionQueue: DispatchQueue
    private let videoOutput: AVCaptureVideoDataOutput
    private var deviceName: String?
    private var micSuffix = ""
    private var side: DeviceSide = .landscapeHomeRight
    private var lastLandscape: DeviceSide = .landscapeHomeRight
    /// Set while live: the stream keeps its portrait or landscape geometry.
    private var lockedPortrait: Bool?
    private var observers: [NSObjectProtocol] = []

    init() {
        let session = AVCaptureSession()
        let videoOutput = AVCaptureVideoDataOutput()
        let sessionQueue = DispatchQueue(label: "capture.session")
        self.session = session
        self.videoOutput = videoOutput
        self.sessionQueue = sessionQueue
        orientationMode = UserDefaults.standard.string(forKey: Self.orientationKey)
            .flatMap(OrientationMode.init(rawValue:)) ?? .auto
        controls = CameraControls(session: session, videoOutput: videoOutput, queue: sessionQueue)
        outputs.genlock = controls.genlock
        outputs.torchProbe = controls.torchProbe
        controls.onFormatChanged = { [weak self] format in
            Task { @MainActor in
                guard let self else {
                    return
                }
                self.format = format
                self.updateStatus()
            }
        }
        controls.onInputChanged = { [weak self] name in
            Task { @MainActor in
                guard let self else {
                    return
                }
                self.deviceName = name
                self.updateStatus()
                self.inputGeneration += 1
            }
        }
        updateOrientation()
    }

    /// Focus and expose at a point of the streamed frame (normalised 0...1),
    /// through the same `focus_at` command the control hub uses.
    func focus(atStreamPoint point: CGPoint) {
        controls.run(command: "focus_at", args: ["x": .number(point.x), "y": .number(point.y)]) { error in
            if let error {
                log.info("Tap to focus failed: \(error, privacy: .public)")
            }
        }
    }

    // MARK: Orientation

    /// Applied at once when not live; while live only once the stream stops
    /// (see `lockStreamOrientation`).
    func setOrientationMode(_ mode: OrientationMode) {
        guard mode != orientationMode else {
            return
        }
        orientationMode = mode
        UserDefaults.standard.set(mode.rawValue, forKey: Self.orientationKey)
        updateOrientation()
    }

    /// How the user interface is turned, from the preview view.
    func setDeviceSide(_ side: DeviceSide) {
        guard side != self.side else {
            return
        }
        // `orientationChangePending` depends on it.
        objectWillChange.send()
        self.side = side
        if !side.isPortrait {
            lastLandscape = side
        }
        updateOrientation()
    }

    /// Keeps the current frame geometry until `unlockStreamOrientation`, so a
    /// running encoder never sees a different frame size. Turning the phone
    /// to the other landscape (or portrait) side still keeps it upright.
    func lockStreamOrientation() {
        objectWillChange.send()
        lockedPortrait = streamOrientation.isPortrait
    }

    func unlockStreamOrientation() {
        objectWillChange.send()
        lockedPortrait = nil
        updateOrientation()
    }

    /// Fixes the capture resolution for a stream (a `resolution` change
    /// waits until `unlockFormat`) and returns the active format. Waits for
    /// the session queue, so the size is the one frames arrive in.
    func lockFormat() -> CaptureFormat {
        let controls = controls
        let locked = sessionQueue.sync { controls.lockFormat() }
        if let locked, locked != format {
            format = locked
            updateStatus()
        }
        return locked ?? format
    }

    func unlockFormat() {
        let controls = controls
        sessionQueue.async { controls.unlockFormat() }
    }

    /// The encoded frame size: the capture format turned to the stream
    /// orientation.
    var streamSize: (width: Int32, height: Int32) {
        streamOrientation.size(landscape: format.landscapeSize)
    }

    /// True while live and the setting asks for the other geometry.
    var orientationChangePending: Bool {
        guard let lockedPortrait else {
            return false
        }
        let wanted = StreamOrientation.resolve(mode: orientationMode, side: side,
                                               lastLandscape: lastLandscape, lockedPortrait: nil)
        return wanted.isPortrait != lockedPortrait
    }

    private func updateOrientation() {
        let orientation = StreamOrientation.resolve(mode: orientationMode, side: side,
                                                    lastLandscape: lastLandscape, lockedPortrait: lockedPortrait)
        guard orientation != streamOrientation else {
            return
        }
        let resized = orientation.isPortrait != streamOrientation.isPortrait
        streamOrientation = orientation
        controls.setRotation(orientation.rotationAngle)
        if resized {
            log.info("Stream orientation \(orientation.isPortrait ? "portrait" : "landscape")")
        }
        updateStatus()
    }

    private func updateStatus() {
        guard let deviceName else {
            return
        }
        let size = streamSize
        status = "\(deviceName) \(size.width)x\(size.height) \(format.frameRate) fps" + micSuffix
    }

    func start() async {
        guard await AVCaptureDevice.requestAccess(for: .video) else {
            status = "Camera access denied"
            cameraError = "Camera access denied. Allow it in Settings > TallyCam."
            return
        }
        guard let device = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back),
              let videoInput = try? AVCaptureDeviceInput(device: device)
        else {
            status = "No back camera"
            cameraError = "No camera available"
            return
        }
        var addedAudio = false
        if await AVCaptureDevice.requestAccess(for: .audio) {
            addedAudio = mic.start()
        } else {
            log.info("Microphone access denied, video only")
        }
        let session = session
        let videoOutput = videoOutput
        let outputs = outputs
        let controls = controls
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            sessionQueue.async {
                session.beginConfiguration()
                // The microphone runs on its own audio unit and session
                // settings; the camera session must not change them.
                session.automaticallyConfiguresApplicationAudioSession = false
                session.usesApplicationAudioSession = true
                // The format comes from `CameraControls` (device.activeFormat).
                if session.canSetSessionPreset(.inputPriority) {
                    session.sessionPreset = .inputPriority
                }
                if session.canAddInput(videoInput) {
                    session.addInput(videoInput)
                }
                videoOutput.videoSettings = [
                    kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                ]
                videoOutput.alwaysDiscardsLateVideoFrames = true
                videoOutput.setSampleBufferDelegate(outputs, queue: outputs.videoQueue)
                if session.canAddOutput(videoOutput) {
                    session.addOutput(videoOutput)
                }
                controls.attach(device: device, input: videoInput, frameRate: 30)
                session.commitConfiguration()
                session.startRunning()
                controls.startPolling()
                continuation.resume()
            }
        }
        hasAudio = addedAudio
        micSuffix = addedAudio ? " + mic" : ", no mic"
        deviceName = device.localizedName
        updateStatus()
        observeSession()
    }

    /// The session pauses itself in the background and while another app
    /// holds the camera, and resumes on its own. Show why; restart it after
    /// a media services reset, which it does not survive.
    private func observeSession() {
        let center = NotificationCenter.default
        let session = session
        observers.append(center.addObserver(forName: AVCaptureSession.wasInterruptedNotification,
                                            object: session, queue: .main)
        { [weak self] note in
            let reason = (note.userInfo?[AVCaptureSessionInterruptionReasonKey] as? Int)
                .flatMap(AVCaptureSession.InterruptionReason.init(rawValue:))
            MainActor.assumeIsolated {
                self?.interrupted(reason)
            }
        })
        observers.append(center.addObserver(forName: AVCaptureSession.interruptionEndedNotification,
                                            object: session, queue: .main)
        { [weak self] _ in
            MainActor.assumeIsolated {
                log.info("Camera interruption ended")
                self?.updateStatus()
            }
        })
        observers.append(center.addObserver(forName: AVCaptureSession.runtimeErrorNotification,
                                            object: session, queue: .main)
        { [weak self] note in
            let error = note.userInfo?[AVCaptureSessionErrorKey] as? AVError
            MainActor.assumeIsolated {
                self?.runtimeError(error)
            }
        })
    }

    private func interrupted(_ reason: AVCaptureSession.InterruptionReason?) {
        let text = switch reason {
        case .videoDeviceNotAvailableInBackground: "app in background"
        case .videoDeviceInUseByAnotherClient: "camera in use by another app"
        case .videoDeviceNotAvailableWithMultipleForegroundApps: "camera unavailable in split view"
        case .videoDeviceNotAvailableDueToSystemPressure: "system pressure (heat)"
        case .audioDeviceInUseByAnotherClient: "audio in use by another app"
        default: "unknown reason"
        }
        log.info("Camera interrupted: \(text, privacy: .public)")
        status = "Camera paused: \(text)"
    }

    private func runtimeError(_ error: AVError?) {
        log.error("Camera runtime error: \(error?.localizedDescription ?? "unknown", privacy: .public)")
        guard error?.code == .mediaServicesWereReset else {
            return
        }
        let session = session
        sessionQueue.async {
            if !session.isRunning {
                session.startRunning()
            }
        }
    }
}

/// Receives camera sample buffers on a dedicated queue and forwards them to
/// whatever sink is installed (the live stream, while live).
final class CaptureOutputs: NSObject, @unchecked Sendable, AVCaptureVideoDataOutputSampleBufferDelegate {
    typealias Sink = (CMSampleBuffer) -> Void
    /// Taps video frames while the stream may be idle (preview images).
    typealias PreviewSink = @Sendable (CMSampleBuffer) -> Void

    let videoQueue = DispatchQueue(label: "capture.video", qos: .userInteractive)
    private let lock = NSLock()
    private var videoSink: Sink?
    private var previewSink: PreviewSink?
    private var dropped = 0
    /// Set once before the session starts.
    var genlock: GenlockController?
    var torchProbe: TorchProbe?

    /// Frames the video output dropped since the start because its queue
    /// was still busy (the encoder not keeping up).
    var droppedFrames: Int {
        lock.withLock { dropped }
    }

    func setVideoSink(_ sink: Sink?) {
        lock.withLock {
            videoSink = sink
        }
    }

    func setPreviewSink(_ sink: PreviewSink?) {
        lock.withLock {
            previewSink = sink
        }
    }

    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer,
                       from _: AVCaptureConnection)
    {
        let (sink, preview) = lock.withLock {
            (videoSink, previewSink)
        }
        // The encoder first: what runs before it adds to every frame's
        // latency.
        sink?(sampleBuffer)
        genlock?.onFrame(sampleBuffer)
        torchProbe?.onFrame(sampleBuffer)
        preview?(sampleBuffer)
    }

    func captureOutput(_: AVCaptureOutput, didDrop _: CMSampleBuffer, from _: AVCaptureConnection) {
        lock.withLock {
            dropped += 1
        }
    }
}
