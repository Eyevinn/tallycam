import Combine
import Foundation
import os
import UIKit

private let log = Logger(subsystem: "se.eyevinn.tallycam", category: "remote")

/// The device side of LCP: connects to the control server, describes the
/// controls, reports state and status, and applies what the server asks for.
@MainActor
final class RemoteControl: ObservableObject {
    enum Tally: String {
        case off
        case preview
        case program
    }

    /// Returns the clock discipline log (chrony's measurements, statistics,
    /// tracking and selection logs, plus events) and its reports.
    static let clockLogCommand = CommandDescriptor(name: "clock_log", label: "Clock log",
                                                   args: ["lines": "1..4000"])
    private static let urlKey = "lcpUrl"
    private static let bannerSeconds: UInt64 = 8
    private static let maxPendingLogs = 50
    private static let logTime: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss.SSS"
        return f
    }()

    @Published var url: String {
        didSet { UserDefaults.standard.set(url, forKey: Self.urlKey) }
    }

    @Published private(set) var connected = false
    @Published private(set) var tally: Tally = .off
    @Published private(set) var banner: String?
    /// The phone's clock discipline, refreshed every second.
    @Published private(set) var clock: ClockSummary?

    enum Link {
        case notSetUp
        case connecting
        case connected
    }

    var link: Link {
        connected ? .connected : isUnconfigured ? .notSetUp : .connecting
    }

    /// Host of the control URL, for display.
    var host: String? {
        isUnconfigured ? nil : URL(string: url.trimmingCharacters(in: .whitespaces))?.host ?? url
    }

    func deviceInfo(key: DeviceKey) -> DeviceInfo? {
        reporter.deviceInfo(key: key)
    }

    /// True when there is no control URL yet (never scanned, never typed).
    var isUnconfigured: Bool {
        url.trimmingCharacters(in: .whitespaces).isEmpty
    }

    private let client: LcpClient
    private let reporter = StatusReporter()
    private let preview = PreviewEncoder()
    private var capture: Capture?
    private var controller: StreamController?
    private var cameraControls: [ControlDescriptor] = []
    private var lastDroppedFrames = 0
    private var droppedSinceWarning = 0
    private var lastDropWarning = Date.distantPast
    /// The stream controls as last sent in `hello`.
    private var streamControls: [ControlDescriptor] = []
    private var cameraCommands: [CommandDescriptor] = []
    private var cameraValues: [String: JSONValue] = [:]
    private var sentValues: [String: JSONValue] = [:]
    /// Admitted: hello answered (and the device key proven), the full state
    /// sent. Status, state deltas and logs go out only then.
    private var helloSent = false
    /// A `challenge` came after this connection's hello: the server checks
    /// device keys, and admission is its first message after a valid `auth`.
    private var challengeSeen = false
    /// Retries `reconnect` while the device key cannot be read yet.
    private var keyWait: (observer: NSObjectProtocol, timer: Timer)?
    /// Hello sent on this connection, waiting for the server to admit the
    /// phone. A server with device keys first sends `challenge`; any server
    /// admits with its first `tally` (a refusal is a `message`, then close).
    private var awaitingAdmission = false
    private var lastStreamState: StreamController.State = .idle
    private var statusTimer: Timer?
    private var bannerTask: Task<Void, Never>?
    /// Pending close after going to the background; cancelled on return.
    private var backgroundClose: Task<Void, Never>?
    /// The channel was closed for the background and must be reopened.
    private var closedForBackground = false
    private var pendingLogs: [LogMessage] = []
    private var subscriptions: Set<AnyCancellable> = []
    /// Host to announce once the control channel opens after a scan.
    private var provisionedHost: String?

    init() {
        url = UserDefaults.standard.string(forKey: Self.urlKey) ?? ""
        client = LcpClient()
    }

    func start(capture: Capture, controller: StreamController) {
        guard self.capture == nil else {
            return
        }
        self.capture = capture
        self.controller = controller
        // The phone keeps its own NTP-disciplined clock (not the server's).
        PhoneClock.shared.start()
        capture.controls.onUpdate = { [weak self] update in
            Task { @MainActor in
                self?.handle(update)
            }
        }
        client.onEvent = { [weak self] event in
            Task { @MainActor in
                self?.handle(event)
            }
        }
        let client = client
        preview.onJpeg = { jpeg, width, height in
            client.send(PreviewMessage(jpeg: jpeg.base64EncodedString(), width: width, height: height), droppable: true)
        }
        reporter.onThermalChange = { [weak self] state in
            self?.sendLog(level: state == "serious" || state == "critical" ? "warn" : "info",
                          message: "Thermal state \(state)")
        }
        controller.objectWillChange
            .receive(on: DispatchQueue.main)
            .sink { [weak self] in self?.streamChanged() }
            .store(in: &subscriptions)
        capture.$format
            .removeDuplicates()
            .receive(on: DispatchQueue.main)
            .sink { [weak controller] format in controller?.setCaptureFormat(format) }
            .store(in: &subscriptions)
        capture.$orientationMode
            .removeDuplicates()
            .receive(on: DispatchQueue.main)
            .sink { [weak self] _ in self?.publishState() }
            .store(in: &subscriptions)
        statusTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in
                self?.tick()
            }
        }
        reconnect()
    }

    /// Connects to `url`, replacing any current connection. Without a URL
    /// the client stays disconnected (no retrying against nothing).
    func reconnect() {
        guard !isUnconfigured else {
            client.disconnect()
            return
        }
        // Before the first unlock after a reboot the Keychain cannot be read.
        // Wait for the stored key rather than connect as another camera; the
        // camera cannot run before an unlock anyway.
        guard DeviceKey.current() != nil else {
            waitForDeviceKey()
            return
        }
        stopWaitingForDeviceKey()
        guard let url = URL(string: url.trimmingCharacters(in: .whitespaces)),
              url.scheme == "ws" || url.scheme == "wss"
        else {
            log.error("Invalid control URL")
            client.disconnect()
            return
        }
        client.connect(to: url)
    }

    private func waitForDeviceKey() {
        guard keyWait == nil else {
            return
        }
        log.info("Device key not readable yet, waiting for unlock")
        showBanner("Unlock the phone to connect")
        let observer = NotificationCenter.default.addObserver(
            forName: UIApplication.protectedDataDidBecomeAvailableNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.reconnect() }
        }
        // The notification only fires on the transition; also look again now and then.
        let timer = Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.reconnect() }
        }
        keyWait = (observer, timer)
    }

    private func stopWaitingForDeviceKey() {
        guard let wait = keyWait else {
            return
        }
        NotificationCenter.default.removeObserver(wait.observer)
        wait.timer.invalidate()
        keyWait = nil
    }

    /// The app is going to the background, where iOS suspends it. Gives the
    /// "stream stopped" log a moment to reach the hub, then closes the channel
    /// so the hub shows the camera offline at once instead of after its
    /// timeout.
    func enteredBackground() {
        let app = UIApplication.shared
        var task = UIBackgroundTaskIdentifier.invalid
        task = app.beginBackgroundTask(withName: "Close control channel") {
            app.endBackgroundTask(task)
        }
        backgroundClose?.cancel()
        backgroundClose = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(500))
            if !Task.isCancelled, let self {
                client.disconnect()
                closedForBackground = true
            }
            app.endBackgroundTask(task)
        }
    }

    /// Back in the foreground: connect right away (no backoff wait).
    func enteredForeground() {
        backgroundClose?.cancel()
        backgroundClose = nil
        if !connected || closedForBackground {
            closedForBackground = false
            reconnect()
        }
    }

    /// Forgets the control server: disconnects and clears the saved URL, so
    /// the app asks for a QR scan again. The stream URL stays.
    func forget() {
        url = ""
        reconnect()
    }

    /// Handles a deep link from the control hub's QR code: saves the control
    /// and SRT URLs and connects. While live, the SRT URL applies on the next
    /// Go Live. Returns false if the link is not a provisioning link.
    @discardableResult
    func provision(from link: URL, controller: StreamController) -> Bool {
        guard let deviceId = StatusReporter.deviceId() else {
            showBanner("Unlock the phone, then scan again")
            return false
        }
        guard let parsed = ProvisioningLink(url: link, deviceId: deviceId) else {
            log.error("Ignoring unusable link \(link.absoluteString, privacy: .public)")
            showBanner("Not a valid connect code")
            return false
        }
        log.info("Provisioned control \(parsed.controlUrl, privacy: .public)")
        url = parsed.controlUrl
        if let srt = parsed.srtUrl {
            controller.url = srt
        }
        provisionedHost = parsed.controlHost
        let applyLater = parsed.srtUrl != nil && controller.state != .idle
        showBanner("Connecting to \(parsed.controlHost)" + (applyLater ? "\nStream URL applies on next Go Live" : ""))
        // Before start(), start() connects with the new URL.
        if capture != nil {
            reconnect()
        }
        return true
    }

    // MARK: Events

    private func handle(_ event: LcpClient.Event) {
        switch event {
        case .opened:
            connected = true
            if let host = provisionedHost {
                provisionedHost = nil
                showBanner("Connected to \(host)")
            }
            awaitingAdmission = true
            challengeSeen = false
            sendHelloMessage()
            // A server before device keys sends no challenge and may send
            // nothing at all until something changes: admitted unless it
            // challenges soon.
            Task { @MainActor [weak self] in
                try? await Task.sleep(for: .seconds(3))
                guard let self, connected, awaitingAdmission, !challengeSeen else {
                    return
                }
                admit()
            }
        case let .closed(reason):
            log.info("Control channel closed: \(reason)")
            connected = false
            helloSent = false
            awaitingAdmission = false
            tally = .off
            capture?.outputs.setPreviewSink(nil)
        case let .message(message):
            handle(message)
        }
    }

    private func handle(_ update: CameraControls.Update) {
        switch update {
        case let .values(values):
            cameraValues = values
            publishState()
        case let .controls(controls, commands, values):
            cameraControls = controls
            cameraCommands = commands
            cameraValues = values
            if helloSent {
                sendHello()
            } else if awaitingAdmission {
                // The server keeps the newest hello until it admits the phone.
                sendHelloMessage()
            }
        case let .log(level, message):
            sendLog(level: level, message: message)
        }
    }

    private func admit() {
        guard awaitingAdmission else {
            return
        }
        awaitingAdmission = false
        client.admitted()
        sendAfterHello()
    }

    private func handle(_ message: LcpIncoming) {
        // Anything but a challenge or a message (a refusal is a `message`,
        // then the socket closes) means the phone was let in.
        switch message {
        case .challenge, .message:
            break
        default:
            admit()
        }
        switch message {
        case let .set(id, values):
            applySet(id: id, values: values)
        case let .command(id, name, args) where name == Self.clockLogCommand.name:
            let lines = Int(args["lines"]?.double ?? 200)
            let kinds = (args["kinds"]?.string ?? "")
                .split(separator: ",")
                .map { $0.trimmingCharacters(in: .whitespaces) }
                .filter { !$0.isEmpty }
            let data = PhoneClock.shared.logReport(kinds: kinds, lines: min(max(lines, 1), 4000))
            if let id {
                client.send(ResultMessage(id: id, ok: true, data: data))
            }
        case let .command(id, name, args):
            capture?.controls.run(command: name, args: args) { [weak self] error in
                Task { @MainActor in
                    self?.sendResult(id: id, errors: error.map { [$0] } ?? [])
                }
            }
        case let .challenge(server, nonce):
            guard awaitingAdmission, let key = DeviceKey.current() else {
                return
            }
            challengeSeen = true
            do {
                client.send(text: try key.authMessage(server: server, nonce: nonce))
            } catch {
                log.error("Cannot answer the challenge: \(String(describing: error), privacy: .public)")
                showBanner("Cannot prove this camera's key")
            }
        case let .tally(state):
            tally = Tally(rawValue: state) ?? .off
        case let .message(text):
            showBanner(text)
        case let .preview(enabled, fps, width):
            if enabled {
                preview.configure(fps: fps, width: width)
                let preview = preview
                capture?.outputs.setPreviewSink { preview.offer($0) }
            } else {
                capture?.outputs.setPreviewSink(nil)
            }
        }
    }

    private func applySet(id: JSONValue?, values: [String: JSONValue]) {
        var errors: [String] = []
        var cameraSet: [String: JSONValue] = [:]
        for (key, value) in values {
            if StreamControls.ids.contains(key) {
                if let error = applyStream(key, value) {
                    errors.append("\(key): \(error)")
                }
            } else {
                cameraSet[key] = value
            }
        }
        guard !cameraSet.isEmpty, let capture else {
            if !cameraSet.isEmpty {
                errors.append("Camera not ready")
            }
            sendResult(id: id, errors: errors)
            publishState()
            return
        }
        capture.controls.apply(cameraSet) { [weak self] cameraErrors in
            Task { @MainActor in
                self?.sendResult(id: id, errors: errors + cameraErrors)
            }
        }
    }

    private func applyStream(_ key: String, _ value: JSONValue) -> String? {
        guard let controller, let capture else {
            return "not ready"
        }
        return StreamControls.apply(key, value, controller: controller, capture: capture)
    }

    private func streamChanged() {
        guard let controller else {
            return
        }
        PhoneClock.shared.setLive(controller.state == .live)
        // The bitrate range follows the format and codec.
        if helloSent, StreamControls.descriptors(controller) != streamControls {
            sendHello()
        }
        if controller.state != lastStreamState {
            switch controller.state {
            case .connecting:
                if let reason = controller.message {
                    sendLog(level: "warn", message: "Connection lost: \(reason), reconnecting")
                } else {
                    sendLog(level: "info", message: "Connecting to \(SrtUrl.redacted(controller.url))")
                }
            case .live: sendLog(level: "info", message: controller.liveVideo.map { "Live, \($0)" } ?? "Live")
            case .idle: sendLog(level: controller.message == nil ? "info" : "error",
                                message: controller.message.map { "Stream stopped: \($0)" } ?? "Stream stopped")
            }
            lastStreamState = controller.state
        }
        publishState()
    }

    // MARK: Sending

    private func currentValues() -> [String: JSONValue] {
        var values = cameraValues
        if let controller {
            values.merge(StreamControls.values(controller, capture: capture)) { _, stream in stream }
        }
        return values
    }

    /// Hello, then everything that follows it on an admitted connection.
    private func sendHello() {
        sendHelloMessage()
        sendAfterHello()
    }

    private func sendHelloMessage() {
        if let controller {
            streamControls = StreamControls.descriptors(controller)
        }
        guard let key = DeviceKey.current() else {
            return
        }
        let hello = HelloMessage(device: reporter.deviceInfo(key: key),
                                 publicKey: key.identity.publicKey,
                                 controls: streamControls + cameraControls,
                                 commands: cameraCommands + [Self.clockLogCommand])
        client.send(hello)
    }

    /// The full state, the logs kept while away, and a status.
    private func sendAfterHello() {
        let values = currentValues()
        client.send(StateMessage(values: values))
        sentValues = values
        helloSent = true
        if !pendingLogs.isEmpty {
            client.send(LogMessage(level: "info", message: "While disconnected:"))
            for message in pendingLogs {
                client.send(message)
            }
            pendingLogs.removeAll()
        }
        sendStatus()
    }

    /// Sends the values that changed since the last `state`.
    private func publishState() {
        guard helloSent else {
            return
        }
        let values = currentValues()
        let delta = values.filter { sentValues[$0.key] != $0.value }
        guard !delta.isEmpty else {
            return
        }
        client.send(StateMessage(values: delta))
        sentValues.merge(delta) { _, new in new }
    }

    private func tick() {
        let clockStatus = PhoneClock.shared.status()
        let summary = clockStatus.map(ClockSummary.init(status:))
        if summary != clock {
            clock = summary
        }
        sendStatus(clock: clockStatus)
        reportDroppedFrames()
    }

    /// Warns, at most every 10 s, while the camera drops frames because the
    /// encoder does not keep up (as H.264 at 2160p60 does). A few frames
    /// around a format or stream start are not worth a warning.
    private func reportDroppedFrames() {
        guard let capture, let controller else {
            return
        }
        let dropped = capture.outputs.droppedFrames
        defer { lastDroppedFrames = dropped }
        guard controller.state == .live else {
            droppedSinceWarning = 0
            return
        }
        droppedSinceWarning += max(dropped - lastDroppedFrames, 0)
        guard Date().timeIntervalSince(lastDropWarning) >= 10 else {
            return
        }
        if droppedSinceWarning >= 10 {
            let format = capture.format
            sendLog(level: "warn", message: "Camera dropped \(droppedSinceWarning) frames: the encoder does not keep up"
                + " with \(format.width)x\(format.height) at \(format.frameRate) fps"
                + (controller.videoCodec == .h264 ? " (HEVC may)" : ""))
        }
        lastDropWarning = Date()
        droppedSinceWarning = 0
    }

    private func sendStatus(clock: JSONValue? = PhoneClock.shared.status()) {
        guard helloSent, let controller else {
            return
        }
        var status = reporter.status(stream: controller)
        status.clock = clock
        status.genlock = capture?.controls.genlock.status?.message
        client.send(status)
    }

    private func sendResult(id: JSONValue?, errors: [String]) {
        if !errors.isEmpty {
            log.info("Request failed: \(errors.joined(separator: "; "))")
        }
        guard let id else {
            return
        }
        client.send(ResultMessage(id: id, ok: errors.isEmpty,
                                  error: errors.isEmpty ? nil : errors.joined(separator: "; ")))
    }

    /// While the channel is down (background, network loss) logs are kept,
    /// marked with their time, and sent after the next hello, so the hub
    /// learns what happened while the phone was away.
    private func sendLog(level: String, message: String) {
        guard helloSent else {
            pendingLogs.append(LogMessage(level: level,
                                          message: "\(Self.logTime.string(from: Date())) \(message)"))
            if pendingLogs.count > Self.maxPendingLogs {
                pendingLogs.removeFirst(pendingLogs.count - Self.maxPendingLogs)
            }
            return
        }
        client.send(LogMessage(level: level, message: message))
    }

    func logLifecycle(_ message: String) {
        log.info("\(message, privacy: .public)")
        sendLog(level: "info", message: message)
    }

    private func showBanner(_ text: String) {
        banner = text
        bannerTask?.cancel()
        bannerTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: Self.bannerSeconds * 1_000_000_000)
            guard !Task.isCancelled else {
                return
            }
            self?.banner = nil
        }
    }
}
