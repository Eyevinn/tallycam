@preconcurrency import AVFoundation
import os

private let log = Logger(subsystem: "se.eyevinn.tallycam", category: "camera-controls")

/// One camera control: how to describe it from the active device, read its
/// current (measured) value and apply a new one. All closures run on the
/// capture session queue. Adding a control is one entry in `CameraControls.table`.
struct CameraControl {
    let id: String
    let label: String
    let group: String
    var enabledWhen: [String: JSONValue]?
    /// nil when the active device does not have this control.
    let kind: (CameraControls) -> ControlKind?
    let get: (CameraControls) -> JSONValue?
    /// Returns a warning (for example "clamped") when applied with changes.
    let set: (CameraControls, JSONValue) throws -> String?
    /// Changing this control can change other controls' ranges or options.
    var reshapes = false
}

/// A one-shot camera action. `run` is called on the session queue and calls
/// `done` (on the session queue) once finished.
struct CameraCommand {
    let descriptor: CommandDescriptor
    let run: (CameraControls, [String: JSONValue], @escaping (Error?) -> Void) -> Void
}

struct ControlError: LocalizedError {
    let message: String

    init(_ message: String) {
        self.message = message
    }

    var errorDescription: String? {
        message
    }
}

/// Camera controls on the active AVCaptureDevice. Everything here is owned by
/// the capture session queue; the public methods hop onto it.
final class CameraControls: @unchecked Sendable {
    enum Update {
        /// Current value of every control (measured in auto modes).
        case values([String: JSONValue])
        /// The control set changed (lens or format switch); resend `hello`.
        case controls([ControlDescriptor], [CommandDescriptor], [String: JSONValue])
        case log(level: String, message: String)
    }

    struct Lens {
        let id: String
        let label: String
        let type: AVCaptureDevice.DeviceType
    }

    /// Called on the session queue.
    var onUpdate: (@Sendable (Update) -> Void)?
    /// Called on the session queue with the new device name after a lens switch.
    var onInputChanged: (@Sendable (String) -> Void)?
    /// Called on the session queue when the active format's size or frame
    /// rate changed.
    var onFormatChanged: (@Sendable (CaptureFormat) -> Void)?

    private static let resolutionKey = "videoResolution"
    static let mainsKey = "mainsHz"

    let session: AVCaptureSession
    let videoOutput: AVCaptureVideoDataOutput
    let queue: DispatchQueue
    let genlock = GenlockController()
    let torchProbe = TorchProbe()
    /// Auto exposure: the shutter stays at the production's (see
    /// `effectiveShutter`) and the core's loop sets the ISO from the metered
    /// exposure. iOS auto exposure would change the shutter (flicker) and the
    /// frame duration, which genlock owns. Under a free shutter the loop
    /// shortens the shutter once the ISO is at its minimum.
    var autoIso = false {
        didSet {
            if autoIso, !oldValue {
                autoIsoLoop.reset()
            }
        }
    }
    var autoIsoBias: Float = 0 {
        didSet { autoIsoLoop.setBias(ev: Double(autoIsoBias)) }
    }
    /// The phone's own mains frequency (50 or 60 Hz, 0 for none), from
    /// Settings: its own production while no hub sends one.
    var localMainsHz: Int {
        didSet { UserDefaults.standard.set(localMainsHz, forKey: Self.mainsKey) }
    }
    /// Mains set on the phone (Settings or `mains_hz`) since the hub's last
    /// production change: the last write wins, and a new production clears
    /// it. Never saved.
    var mainsOverride: Int?
    /// The hub's production (docs/SHADING.md, "Shutter"); nil without a hub,
    /// when the phone is its own production: one mains period, or free when
    /// its mains is off. Never saved.
    private(set) var serverProduction: Production?
    /// A production that arrived while live (nil inside: back to the phone's
    /// own), applied when the stream stops. A later one replaces it.
    private var pendingProduction: Production??
    /// The frame rate the loop's free shutter base was seeded for; nil while
    /// the shutter is not free.
    private var freeShutterSeededFps: Int?
    private let autoIsoLoop: AutoIsoLoop
    /// An auto ISO exposure is being applied: no samples until the device has
    /// it, since `iso` and `exposureDuration` lag behind what was commanded.
    private var autoIsoInFlight = false
    private var autoIsoLimit = AutoIsoLoop.Limit.none
    private var pollTicks = 0

    // Owned by `queue`.
    private(set) var device: AVCaptureDevice?
    private var input: AVCaptureDeviceInput?
    private(set) var lenses: [Lens] = []
    private(set) var rotationAngle: CGFloat = 0
    var stabilization: AVCaptureVideoStabilizationMode = .off
    /// Last requested manual white balance, so setting temperature keeps tint.
    var manualWhiteBalance = AVCaptureDevice.WhiteBalanceTemperatureAndTintValues(temperature: 5000, tint: 0)
    /// ISO and shutter asked for in the `apply` being run; committed at its end.
    var exposureRequest = ManualExposureRequest()
    /// The last committed manual exposure, reported in `values()` until the
    /// device has applied it.
    private var exposureInFlight: (request: ManualExposureRequest, serial: Int)?
    private var exposureSerial = 0
    private var descriptors: [ControlDescriptor] = []
    private var pollTimer: DispatchSourceTimer?
    /// `CameraButtonControls` (iOS 18), the Camera Control button's sliders.
    private var cameraButton: AnyObject?
    /// The operator's resolution; the lens may not have it (`attach`,
    /// `switchLens` fall back).
    private(set) var wantedResolution: VideoResolution
    /// Set while live: resolution, frame rate and the production shutter
    /// cannot change (docs/SHADING.md, D11).
    private(set) var formatLocked = false
    private var reportedFormat: CaptureFormat?

    init(session: AVCaptureSession, videoOutput: AVCaptureVideoDataOutput, queue: DispatchQueue) {
        self.session = session
        self.videoOutput = videoOutput
        self.queue = queue
        wantedResolution = UserDefaults.standard.string(forKey: Self.resolutionKey)
            .flatMap(VideoResolution.init(rawValue:)) ?? .defaultResolution
        // 0 is "off", so tell an unset value from a stored 0.
        let storedMains = UserDefaults.standard.object(forKey: Self.mainsKey) as? Int
        let mains = storedMains.flatMap { [0, 50, 60].contains($0) ? $0 : nil }
            ?? Int(mainsHzForRegion(region: Locale.current.region?.identifier ?? ""))
        localMainsHz = mains
        autoIsoLoop = AutoIsoLoop(mainsHz: mains)
        genlock.apply = { [weak self] durationNs, serial in
            guard let self else {
                return
            }
            queue.async { self.applyGenlockDuration(durationNs, serial: serial) }
        }
        genlock.onStateChange = { [weak self] state, status in
            let level = state == .locked || state == .acquiring ? "info" : "warn"
            self?.onUpdate?(.log(level: level, message: String(
                format: "Genlock %@: phase error %.2f ms, correction %.1f ppm",
                state.rawValue, status.phaseErrorMs, status.correctionPpm
            )))
        }
    }

    /// Applies the genlock loop's frame duration, keeping the format. Skipped
    /// while the device or format is being changed; the loop then starts
    /// over from `notifyFormatIfChanged`. A duration from a loop older than
    /// the last format or frame rate change (queued before it) is dropped.
    private func applyGenlockDuration(_ durationNs: Double, serial: Int) {
        guard genlock.isCurrent(serial: serial), let device else {
            return
        }
        var duration = CMTime(value: CMTimeValue(durationNs.rounded()), timescale: 1_000_000_000)
        // Clamp as CMTime: 1/30 s in whole ns is just below a 30 fps
        // format's minimum and would be refused.
        let nominal = device.activeVideoMinFrameDuration
        let range = device.activeFormat.videoSupportedFrameRateRanges.first {
            $0.minFrameDuration <= nominal && nominal <= $0.maxFrameDuration
        } ?? device.activeFormat.videoSupportedFrameRateRanges.first
        if let range {
            duration = CMTimeMaximum(range.minFrameDuration, CMTimeMinimum(range.maxFrameDuration, duration))
        }
        let fits = device.activeFormat.videoSupportedFrameRateRanges.contains {
            $0.minFrameDuration <= duration && duration <= $0.maxFrameDuration
        }
        guard fits else {
            return
        }
        do {
            try configure {
                $0.activeVideoMinFrameDuration = duration
                $0.activeVideoMaxFrameDuration = duration
            }
        } catch {
            log.error("Genlock: cannot set the frame duration: \(error.localizedDescription, privacy: .public)")
        }
    }

    /// One auto ISO sample (8 per second): the core's loop decides; this
    /// only reads the metering and applies what it returns.
    private func stepAutoIso() {
        guard autoIso, !autoIsoInFlight, let device else {
            return
        }
        guard device.exposureMode == .custom else {
            autoIso = false
            return
        }
        let format = device.activeFormat
        let period = currentFrameRate.map { 1e9 / Double($0) } ?? device.activeVideoMinFrameDuration.seconds * 1e9
        let input = AutoIsoLoop.Input(
            tNs: HostClock.nowNs(), offsetEv: Double(device.exposureTargetOffset), iso: Double(device.iso),
            exposureNs: device.exposureDuration.seconds * 1e9, minIso: Double(format.minISO),
            maxIso: Double(format.maxISO), minExposureNs: format.minExposureDuration.seconds * 1e9,
            maxExposureNs: AutoIsoLoop.longestExposureNs(framePeriodNs: period,
                                                         formatMaxNs: format.maxExposureDuration.seconds * 1e9,
                                                         genlock: genlock.isEnabled)
        )
        let command = autoIsoLoop.onSample(input)
        let status = autoIsoLoop.status
        if status.limit != autoIsoLimit {
            autoIsoLimit = status.limit
            if status.limit != .none {
                let hint = status.limit == .tooDark ? "too dark: use a longer shutter or more light"
                    : mainsHz == 0 ? "too bright: use less light or an ND filter"
                    : "too bright even at the shortest flicker-free shutter for mains light; outdoors set mains off "
                    + (serverProduction == nil ? "in Settings" : "in the hub's production or in Settings")
                onUpdate?(.log(level: "warn", message: String(format: "Auto ISO at ISO %.0f, %@", device.iso, hint)))
            }
        }
        guard let command else {
            return
        }
        let iso = Float(command.iso)
        let duration = CMTime(value: CMTimeValue(command.exposureNs.rounded()), timescale: 1_000_000_000)
        log.debug("""
        Auto ISO: offset \(input.offsetEv, privacy: .public) EV, ISO \(input.iso, privacy: .public) -> \(command.iso, privacy: .public), \
        shutter \(input.exposureNs / 1e3, privacy: .public) -> \(command.exposureNs / 1e3, privacy: .public) us, \
        gain \(status.gain, privacy: .public)
        """)
        do {
            try configure { device in
                autoIsoInFlight = true
                device.setExposureModeCustom(duration: duration,
                                             iso: min(max(iso, format.minISO), format.maxISO)) { [weak self] _ in
                    self?.queue.async { self?.autoIsoInFlight = false }
                }
            }
        } catch {
            autoIsoInFlight = false
        }
    }

    /// Custom exposure at the values auto exposure reports. With
    /// `currentISO` the picture dropped about 1.3 EV on an iPhone 16
    /// (ISO 377 became 153) and auto ISO had to climb back.
    func enterAutoIso(_ device: AVCaptureDevice) {
        enterCustom(device)
        autoIso = true
        configureLoopShutter()
    }

    /// Custom exposure at the production shutter, with the ISO moved so the
    /// picture keeps its brightness. Under a free shutter the current one
    /// stays.
    func enterCustom(_ device: AVCaptureDevice) {
        let format = device.activeFormat
        let current = device.exposureDuration.seconds * 1e9
        guard let target = productionExposureNs(device), current > 0, abs(target / current - 1) > 0.005 else {
            device.setExposureModeCustom(duration: AVCaptureDevice.currentExposureDuration,
                                         iso: min(max(device.iso, format.minISO), format.maxISO))
            return
        }
        let iso = Float(Double(device.iso) * current / target)
        device.setExposureModeCustom(duration: CMTime(value: CMTimeValue(target.rounded()), timescale: 1_000_000_000),
                                     iso: min(max(iso, format.minISO), format.maxISO))
    }

    // MARK: Production shutter

    struct Production: Equatable {
        var mainsHz: Int
        var shutter: ShutterPolicy
    }

    /// Mains frequency (50 or 60 Hz, 0 for none) for flicker-free shutters:
    /// set on the phone since the production last changed, else the hub's
    /// production's while one applies, else the phone's own.
    var mainsHz: Int {
        mainsOverride ?? serverProduction?.mainsHz ?? localMainsHz
    }

    /// What the shutter follows now: free when the production says so, or
    /// when it follows mains and mains is off.
    var effectiveShutter: ShutterPolicy {
        let policy = serverProduction?.shutter ?? .mains
        return policy == .mains && mainsHz == 0 ? .free : policy
    }

    /// The production's exposure duration for the active format, or nil
    /// under a free shutter. A shutter longer than the frame becomes the
    /// longest flicker-free one that fits (1/100 for 1/50 at 60 fps).
    func productionExposureNs(_ device: AVCaptureDevice) -> Double? {
        let wanted: Double
        switch effectiveShutter {
        case .free: return nil
        case .mains: wanted = 1e9 / Double(mainsHz)
        case let .fixed(denominator): wanted = 1e9 / Double(denominator)
        }
        let format = device.activeFormat
        let period = currentFrameRate.map { 1e9 / Double($0) } ?? device.activeVideoMinFrameDuration.seconds * 1e9
        let longest = AutoIsoLoop.longestExposureNs(framePeriodNs: period,
                                                    formatMaxNs: format.maxExposureDuration.seconds * 1e9,
                                                    genlock: genlock.isEnabled)
        let shortest = format.minExposureDuration.seconds * 1e9
        guard wanted > longest else {
            return max(wanted, shortest)
        }
        if mainsHz > 0 {
            let pulse = 1e9 / Double(2 * mainsHz)
            let pulses = (longest / pulse).rounded(.down)
            if pulses >= 1 {
                return max(pulses * pulse, shortest)
            }
        }
        return max(longest, shortest)
    }

    /// Tells the auto ISO loop how to treat the shutter.
    private func configureLoopShutter() {
        switch effectiveShutter {
        case .mains:
            autoIsoLoop.setMains(hz: mainsHz)
            autoIsoLoop.setFreeShutter(baseNs: 0)
        case .fixed:
            // The production chose this value on purpose: no rounding.
            autoIsoLoop.setMains(hz: 0)
            autoIsoLoop.setFreeShutter(baseNs: 0)
        case .free:
            autoIsoLoop.setMains(hz: 0)
            // A 180° shutter, half the frame period (1/60 at 30 fps). Not
            // the device's current shutter, which the loop may already have
            // shortened; seeded again only when the frame rate changes.
            let fps = currentFrameRate ?? 30
            if freeShutterSeededFps != fps {
                freeShutterSeededFps = fps
                autoIsoLoop.setFreeShutter(baseNs: 1e9 / Double(fps) / 2)
            }
            return
        }
        freeShutterSeededFps = nil
    }

    /// The hub's production settings (nil: none, the phone's own). Kept for
    /// the next stream while live. Returns a warning.
    func setProduction(_ next: Production?) -> String? {
        if formatLocked {
            guard next != serverProduction else {
                pendingProduction = nil
                return nil
            }
            let changed = pendingProduction.map { $0 != next } ?? true
            pendingProduction = .some(next)
            return changed ? "production changed while live: kept until the stream stops" : nil
        }
        pendingProduction = nil
        let before = (effectiveShutter, mainsHz)
        if next != serverProduction {
            // A new production: its mains applies again.
            mainsOverride = nil
        }
        serverProduction = next
        guard (effectiveShutter, mainsHz) != before else {
            // Every reconnect sends the production again: no exposure jump
            // when nothing changed (it would undo auto ISO's steps).
            publishControlsIfChanged(force: false)
            return nil
        }
        applyProductionShutter()
        return nil
    }

    /// Moves a custom exposure (auto or manual) to the production shutter.
    func applyProductionShutter() {
        configureLoopShutter()
        if let device, device.exposureMode == .custom {
            _ = try? configure { enterCustom($0) }
        }
        publishControlsIfChanged(force: false)
        onUpdate?(.values(values()))
    }

    /// Genlock picks a format with headroom above the frame rate; off goes
    /// back to the usual format and the nominal frame duration. iOS auto
    /// exposure overrides the frame duration, so genlock moves it to auto ISO
    /// at the shutter auto exposure had chosen.
    func setGenlock(_ on: Bool) throws {
        genlock.setEnabled(on)
        if on, let device, device.exposureMode == .continuousAutoExposure || device.exposureMode == .autoExpose {
            try configure { enterAutoIso($0) }
            onUpdate?(.log(level: "info", message: String(
                format: "Genlock: exposure moved to auto ISO at 1/%.0f%@ "
                    + "(iOS auto exposure sets the frame duration itself)",
                1 / device.exposureDuration.seconds,
                mainsHz == 0 ? "" : ", rounded flicker-free for \(mainsHz) Hz mains"
            )))
        }
        if let frameRate = currentFrameRate {
            session.beginConfiguration()
            defer { session.commitConfiguration() }
            try configure { try self.applyFrameRate(frameRate, on: $0) }
            revalidateStabilization()
        }
        configureGenlock()
    }

    #if DEBUG
    /// `frame_duration` command: a fixed duration on a format with headroom.
    func setTestFrameDuration(us: Double) throws {
        guard !genlock.isEnabled else {
            throw ControlError("turn genlock off first")
        }
        guard let device, let resolution = activeResolution else {
            throw ControlError("no camera")
        }
        // The frame rate the operator chose, not one rounded from a test
        // duration (34 ms would pick 29 fps formats).
        let frameRate = reportedFormat?.frameRate ?? currentFrameRate ?? 30
        let infos = device.formats.map(Self.info)
        guard let index = CameraFormats.best(infos, resolution: resolution, frameRate: frameRate, headroom: true) else {
            throw ControlError("no format")
        }
        let duration = CMTime(value: CMTimeValue((us * 1000).rounded()), timescale: 1_000_000_000)
        // AVFoundation throws (and aborts the app) on a duration outside the
        // format's ranges.
        guard device.formats[index].videoSupportedFrameRateRanges.contains(where: {
            $0.minFrameDuration <= duration && duration <= $0.maxFrameDuration
        }) else {
            throw ControlError("\(us) us is outside the format's frame rate ranges")
        }
        session.beginConfiguration()
        defer { session.commitConfiguration() }
        try configure { device in
            if device.activeFormat != device.formats[index] {
                device.activeFormat = device.formats[index]
            }
            device.activeVideoMinFrameDuration = duration
            device.activeVideoMaxFrameDuration = duration
        }
        let ranges = device.activeFormat.videoSupportedFrameRateRanges
            .map { String(format: "%.0f-%.0f fps", $0.minFrameRate, $0.maxFrameRate) }
        onUpdate?(.log(level: "info", message: String(
            format: "Test frame duration %.2f us: device min %.2f max %.2f us, format %@, exposure %.0f us",
            us, device.activeVideoMinFrameDuration.seconds * 1e6, device.activeVideoMaxFrameDuration.seconds * 1e6,
            ranges.joined(separator: ","), device.exposureDuration.seconds * 1e6
        )))
    }
    #endif

    private func configureGenlock() {
        guard let device, let frameRate = currentFrameRate else {
            return
        }
        let shortest = device.activeFormat.videoSupportedFrameRateRanges.map(\.minFrameDuration.seconds).min() ?? 0
        genlock.configure(.init(fps: frameRate, minFrameDurationNs: shortest * 1e9))
    }

    // MARK: Session queue only

    /// Called on the session queue inside the initial session configuration.
    func attach(device: AVCaptureDevice, input: AVCaptureDeviceInput, frameRate: Int) {
        self.device = device
        self.input = input
        lenses = Self.discoverLenses()
        do {
            _ = try configure { try self.applyFallbackFormat(resolution: wantedResolution, frameRate: frameRate, on: $0) }
        } catch {
            log.error("No capture format: \(error.localizedDescription, privacy: .public)")
        }
        enterDefaultExposure()
        applyConnectionSettings()
    }

    /// Auto exposure as the app means it: auto ISO at the production shutter,
    /// or iOS auto exposure on a camera without custom exposure.
    private func enterDefaultExposure() {
        _ = try? configure { device in
            if device.isExposureModeSupported(.custom) {
                enterAutoIso(device)
            } else if device.isExposureModeSupported(.continuousAutoExposure) {
                device.exposureMode = .continuousAutoExposure
            }
        }
    }

    /// Called on the session queue once the session runs.
    func startPolling() {
        publishControlsIfChanged(force: true)
        notifyFormatIfChanged()
        rebuildCameraButton()
        let timer = DispatchSource.makeTimerSource(queue: queue)
        // Auto ISO samples at 8 Hz; the values go out at 4 Hz.
        timer.schedule(deadline: .now() + 0.125, repeating: 0.125, leeway: .milliseconds(20))
        timer.setEventHandler { [weak self] in
            guard let self else {
                return
            }
            stepAutoIso()
            pollTicks += 1
            if pollTicks % 2 == 0 {
                let values = values()
                onUpdate?(.values(values))
                if #available(iOS 18.0, *) {
                    (cameraButton as? CameraButtonControls)?.refresh(values: values)
                }
            }
        }
        pollTimer = timer
        timer.resume()
    }

    func values() -> [String: JSONValue] {
        var values: [String: JSONValue] = [:]
        guard device != nil else {
            return values
        }
        for control in Self.table where control.kind(self) != nil {
            values[control.id] = control.get(self)
        }
        if let exposureInFlight {
            values.merge(exposureInFlight.request.stateValues()) { _, target in target }
        }
        return values
    }

    func currentDescriptors() -> [ControlDescriptor] {
        guard device != nil else {
            return []
        }
        return Self.table.compactMap { control in
            control.kind(self).map {
                ControlDescriptor(id: control.id, label: control.label, group: control.group,
                                  kind: $0, enabledWhen: control.enabledWhen)
                    .readonly(because: readonlyReason(control.id))
                    .lockedWhileLive(control.id == "resolution" || control.id == "frame_rate"
                        || control.id == "mains_hz")
            }
        }
    }

    /// Why a control is shown but not set, if it is (`readonly_reason`).
    private func readonlyReason(_ id: String) -> String? {
        switch id {
        case "shutter" where effectiveShutter != .free:
            // The production's (docs/SHADING.md, "Shutter").
            "set by the production"
        case "iso" where device?.isExposureModeSupported(.custom) == false:
            // Manual on such a camera is a lock at the values auto had.
            "no manual exposure on this camera"
        default:
            nil
        }
    }

    /// Locks the active device for configuration around `body`.
    func configure<T>(_ body: (AVCaptureDevice) throws -> T) throws -> T {
        guard let device else {
            throw ControlError("No camera")
        }
        try device.lockForConfiguration()
        defer { device.unlockForConfiguration() }
        return try body(device)
    }

    /// Stabilization modes depend on the active format.
    func revalidateStabilization() {
        if let device, !device.activeFormat.isVideoStabilizationModeSupported(stabilization) {
            stabilization = .off
        }
        applyConnectionSettings()
    }

    /// Converts a point in the streamed (rotated) frame to the device's
    /// point-of-interest space, which is the unrotated sensor frame.
    func devicePoint(x: Double, y: Double) -> CGPoint {
        StreamOrientation.devicePoint(x: x, y: y, angle: rotationAngle)
    }

    /// The formats of the active lens, as format selection sees them.
    var formatInfos: [CameraFormatInfo] {
        device.map { $0.formats.map(Self.info) } ?? []
    }

    var availableResolutions: [VideoResolution] {
        CameraFormats.resolutions(formatInfos)
    }

    var activeResolution: VideoResolution? {
        device.flatMap { Self.info($0.activeFormat).resolution }
    }

    var activeFormat: CaptureFormat? {
        guard let device, let frameRate = currentFrameRate else {
            return nil
        }
        let dimensions = CMVideoFormatDescriptionGetDimensions(device.activeFormat.formatDescription)
        return CaptureFormat(width: dimensions.width, height: dimensions.height, frameRate: frameRate)
    }

    /// The frame rates at the active resolution.
    var frameRateOptions: [CameraFormats.FrameRateOption] {
        activeResolution.map { CameraFormats.frameRates(formatInfos, resolution: $0) } ?? []
    }

    /// Changes the frame rate at the active resolution, switching format
    /// when another one is better for it (binned for high rates and back).
    func applyFrameRate(_ frameRate: Int, on device: AVCaptureDevice) throws {
        guard let resolution = activeResolution else {
            throw ControlError("no active resolution")
        }
        try applyFormat(resolution: resolution, frameRate: frameRate, on: device)
    }

    /// The best format for `resolution` at `frameRate` (see `CameraFormats.best`).
    func applyFormat(resolution: VideoResolution, frameRate: Int, on device: AVCaptureDevice) throws {
        let formats = device.formats
        guard let index = CameraFormats.best(formats.map(Self.info), resolution: resolution, frameRate: frameRate,
                                             headroom: genlock.isEnabled)
        else {
            throw ControlError("\(frameRate) fps not supported at \(resolution.rawValue)")
        }
        if device.activeFormat != formats[index] {
            device.activeFormat = formats[index]
        }
        let duration = CMTime(value: 1, timescale: CMTimeScale(frameRate))
        device.activeVideoMinFrameDuration = duration
        device.activeVideoMaxFrameDuration = duration
    }

    /// `resolution` and `frameRate`, or the nearest the lens has. Returns
    /// what was applied.
    @discardableResult
    func applyFallbackFormat(resolution: VideoResolution, frameRate: Int,
                             on device: AVCaptureDevice) throws -> (VideoResolution, Int)
    {
        let infos = device.formats.map(Self.info)
        guard let resolution = CameraFormats.fallback(resolution, available: CameraFormats.resolutions(infos)),
              let frameRate = CameraFormats.fallback(frameRate: frameRate,
                                                     available: CameraFormats.frameRates(infos, resolution: resolution)
                                                         .map(\.frameRate))
        else {
            throw ControlError("no usable format on \(device.localizedName)")
        }
        try applyFormat(resolution: resolution, frameRate: frameRate, on: device)
        return (resolution, frameRate)
    }

    /// The `resolution` control. Refused while live (`formatLocked`).
    func setResolution(_ resolution: VideoResolution) throws -> String? {
        guard !formatLocked || resolution == activeResolution else {
            throw ControlError("not while live: stop the stream to change it")
        }
        wantedResolution = resolution
        UserDefaults.standard.set(resolution.rawValue, forKey: Self.resolutionKey)
        guard resolution != activeResolution else {
            return nil
        }
        let frameRate = currentFrameRate ?? 30
        session.beginConfiguration()
        defer { session.commitConfiguration() }
        let applied = try configure { try applyFallbackFormat(resolution: resolution, frameRate: frameRate, on: $0) }
        revalidateStabilization()
        log.info("Resolution \(applied.0.rawValue, privacy: .public) at \(applied.1) fps")
        return applied.1 == frameRate ? nil : "\(frameRate) fps not available at \(resolution.rawValue), using \(applied.1)"
    }

    /// Fixes the frame size for a stream and returns it. Frame rate changes
    /// within the resolution stay possible.
    func lockFormat() -> CaptureFormat? {
        formatLocked = true
        return activeFormat
    }

    /// The stream stopped: applies a production that came while live.
    func unlockFormat() {
        formatLocked = false
        guard let pending = pendingProduction else {
            return
        }
        pendingProduction = nil
        _ = setProduction(pending)
        onUpdate?(.log(level: "info", message: "Production shutter applied after the stream stopped"))
    }

    func switchLens(to id: String) throws {
        guard let lens = lenses.first(where: { $0.id == id }),
              let newDevice = AVCaptureDevice.default(lens.type, for: .video, position: .back)
        else {
            throw ControlError("No lens \(id)")
        }
        guard newDevice != device else {
            return
        }
        let frameRate = currentFrameRate ?? 30
        // While live the frame size must stay; otherwise keep the operator's
        // resolution where the lens has it.
        let resolution = formatLocked ? activeResolution ?? wantedResolution : wantedResolution
        if formatLocked, !CameraFormats.resolutions(newDevice.formats.map(Self.info)).contains(resolution) {
            throw ControlError("The \(lens.label) lens has no \(resolution.rawValue); stop the stream to switch")
        }
        // Exposure, colour and focus keep their modes and values on the new
        // lens (someone set them, docs/SHADING.md); the shutter too when it
        // is free (an exposure control then).
        let free = effectiveShutter == .free
        let carried = values().filter { Self.carriedOverLensSwitch.contains($0.key) || free && $0.key == "shutter" }
        let newInput = try AVCaptureDeviceInput(device: newDevice)
        session.beginConfiguration()
        if let input {
            session.removeInput(input)
        }
        guard session.canAddInput(newInput) else {
            if let input {
                session.addInput(input)
            }
            session.commitConfiguration()
            throw ControlError("Cannot use the \(lens.label) lens")
        }
        session.addInput(newInput)
        device = newDevice
        input = newInput
        var warning: String?
        do {
            let applied = try configure {
                try self.applyFallbackFormat(resolution: resolution, frameRate: frameRate, on: $0)
            }
            if applied != (resolution, frameRate) {
                warning = "\(lens.label) lens: \(applied.0.rawValue) at \(applied.1) fps"
                    + " (has no \(resolution.rawValue) at \(frameRate) fps)"
            }
        } catch {
            warning = "\(lens.label) lens: \(error.localizedDescription)"
        }
        revalidateStabilization()
        session.commitConfiguration()
        autoIso = false
        enterDefaultExposure()
        for control in Self.table where carried[control.id] != nil && control.kind(self) != nil {
            _ = try? control.set(self, carried[control.id]!)
        }
        // ISO and shutter are applied together (ManualExposureRequest).
        _ = commitExposure()
        if let warning {
            onUpdate?(.log(level: "warn", message: warning))
        }
        log.info("Switched to \(newDevice.localizedName)")
        rebuildCameraButton()
        onInputChanged?(newDevice.localizedName)
        onUpdate?(.log(level: "info", message: "Switched to \(lens.label) lens (\(newDevice.localizedName))"))
    }

    /// Rotation and stabilization live on the output connection, which is
    /// recreated when the input changes.
    func applyConnectionSettings() {
        guard let connection = videoOutput.connection(with: .video) else {
            return
        }
        if connection.isVideoRotationAngleSupported(rotationAngle) {
            connection.videoRotationAngle = rotationAngle
        }
        if connection.isVideoStabilizationSupported {
            connection.preferredVideoStabilizationMode = stabilization
        }
    }

    var currentFrameRate: Int? {
        guard let duration = device?.activeVideoMinFrameDuration, duration.isValid, duration.seconds > 0 else {
            return nil
        }
        return Int((1 / duration.seconds).rounded())
    }

    // MARK: Any thread

    func setRotation(_ angle: CGFloat) {
        queue.async { [self] in
            rotationAngle = angle
            applyConnectionSettings()
        }
    }

    /// Smooth zoom from the operator's rocker: ramps towards the longest
    /// (`rate` > 0) or widest (`rate` < 0) zoom at `abs(rate)` doublings per
    /// second, until `stopZoomRamp` or a new rate.
    func rampZoom(rate: Double) {
        queue.async { [self] in
            _ = try? configure { device in
                guard abs(rate) > 0.001 else {
                    device.cancelVideoZoomRamp()
                    return
                }
                let target = rate > 0
                    ? min(device.maxAvailableVideoZoomFactor, 10)
                    : device.minAvailableVideoZoomFactor
                device.ramp(toVideoZoomFactor: target, withRate: Float(abs(rate)))
            }
        }
    }

    /// Stops a zoom ramp where it is and publishes the zoom it reached.
    func stopZoomRamp() {
        queue.async { [self] in
            _ = try? configure { $0.cancelVideoZoomRamp() }
            onUpdate?(.values(values()))
        }
    }

    /// Applies values in table order (modes before the values that depend on
    /// them) and completes with one error message per failed or clamped value.
    /// Sets controls; `completion` gets the errors and warnings, and the ids
    /// that took effect (clamped included, refused not).
    func apply(_ values: [String: JSONValue], completion: @escaping @Sendable ([String], Set<String>) -> Void) {
        queue.async { [self] in
            var errors: [String] = []
            var applied: Set<String> = []
            var reshaped = false
            for key in values.keys where !Self.table.contains(where: { $0.id == key }) {
                errors.append("Unknown control \(key)")
            }
            for control in Self.table {
                guard let value = values[control.id] else {
                    continue
                }
                do {
                    guard control.kind(self) != nil else {
                        throw ControlError("not available on this lens")
                    }
                    if let warning = try control.set(self, value) {
                        errors.append("\(control.id): \(warning)")
                    }
                    applied.insert(control.id)
                    reshaped = reshaped || control.reshapes
                } catch {
                    errors.append("\(control.id): \(error.localizedDescription)")
                }
            }
            if let error = commitExposure() {
                errors.append(error)
            }
            if reshaped {
                publishControlsIfChanged(force: false)
                notifyFormatIfChanged()
            }
            onUpdate?(.values(self.values()))
            completion(errors, applied)
        }
    }

    func run(command name: String, args: [String: JSONValue], completion: @escaping @Sendable (String?) -> Void) {
        queue.async { [self] in
            guard let command = Self.commands.first(where: { $0.descriptor.name == name }) else {
                completion("Unknown command \(name)")
                return
            }
            command.run(self, args) { error in
                completion(error?.localizedDescription)
            }
        }
    }

    // MARK: Private

    /// Applies the ISO and shutter of this `apply` in one call; when the
    /// device has applied them, publishes the values again. Returns an error.
    private func commitExposure() -> String? {
        let request = exposureRequest
        exposureRequest = ManualExposureRequest()
        guard !request.isEmpty else {
            return nil
        }
        exposureSerial += 1
        let serial = exposureSerial
        do {
            try configure { device in
                let target = request.resolved()
                exposureInFlight = (request, serial)
                device.setExposureModeCustom(duration: target.duration, iso: target.iso) { [weak self] _ in
                    guard let self else {
                        return
                    }
                    queue.async {
                        if self.exposureInFlight?.serial == serial {
                            self.exposureInFlight = nil
                        }
                        self.onUpdate?(.values(self.values()))
                    }
                }
            }
            return nil
        } catch {
            return "exposure: \(error.localizedDescription)"
        }
    }

    private func notifyFormatIfChanged() {
        guard let format = activeFormat, format != reportedFormat else {
            return
        }
        reportedFormat = format
        log.info("Capture format \(format.width)x\(format.height) at \(format.frameRate) fps")
        configureGenlock()
        rebuildCameraButton()
        onFormatChanged?(format)
    }

    /// The Camera Control button's sliders for the current device and format
    /// (iPhone 16 and later; nothing elsewhere).
    private func rebuildCameraButton() {
        guard #available(iOS 18.0, *), session.supportsControls else {
            return
        }
        let button = cameraButton as? CameraButtonControls
            ?? CameraButtonControls(session: session, queue: queue, controls: self)
        cameraButton = button
        button.rebuild(device: device, values: values())
    }

    private func publishControlsIfChanged(force: Bool) {
        let descriptors = currentDescriptors()
        guard force || descriptors != self.descriptors else {
            return
        }
        self.descriptors = descriptors
        onUpdate?(.controls(descriptors, Self.commands.map(\.descriptor), values()))
    }

    /// Mode and value controls of the exposure, colour and focus groups.
    private static let carriedOverLensSwitch: Set<String> = [
        "exposure_mode", "iso", "ev_bias", "wb_mode", "wb_temperature", "wb_tint", "focus_mode", "focus_position",
    ]

    private static func discoverLenses() -> [Lens] {
        let candidates: [Lens] = [
            Lens(id: "ultra_wide", label: "Ultra wide", type: .builtInUltraWideCamera),
            Lens(id: "wide", label: "Wide", type: .builtInWideAngleCamera),
            Lens(id: "tele", label: "Tele", type: .builtInTelephotoCamera),
        ]
        let found = AVCaptureDevice.DiscoverySession(deviceTypes: candidates.map(\.type),
                                                     mediaType: .video, position: .back).devices
        let usable = candidates.compactMap { lens in
            found.first { $0.deviceType == lens.type && !CameraFormats.resolutions($0.formats.map(info)).isEmpty }
                .map { (lens, $0) }
        }
        // The label carries the zoom factor against the wide lens, from the
        // field of view ("Tele 5×"), so the operator sees which tele it is.
        let wideFov = usable.first { $0.0.id == "wide" }?.1.activeFormat.videoFieldOfView
        return usable.map { lens, device in
            guard let wideFov, wideFov > 0, device.activeFormat.videoFieldOfView > 0 else {
                return lens
            }
            let ratio = tan(Double(wideFov) * .pi / 360) / tan(Double(device.activeFormat.videoFieldOfView) * .pi / 360)
            // 0.5× and whole factors above 1, as Apple labels its lenses.
            let factor = ratio < 1 ? (ratio * 10).rounded() / 10 : ratio.rounded()
            let text = factor == factor.rounded() ? "\(Int(factor))×" : "\(factor)×"
            return Lens(id: lens.id, label: "\(lens.label) \(text)", type: lens.type)
        }
    }

    static func info(_ format: AVCaptureDevice.Format) -> CameraFormatInfo {
        let dimensions = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
        let subtype = CMFormatDescriptionGetMediaSubType(format.formatDescription)
        return CameraFormatInfo(
            width: dimensions.width, height: dimensions.height,
            is8Bit420: subtype == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
                || subtype == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
            isVideoRange: subtype == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            isBinned: format.isVideoBinned,
            rateRanges: format.videoSupportedFrameRateRanges.map {
                CameraFormatInfo.RateRange(min: $0.minFrameRate, max: $0.maxFrameRate)
            }
        )
    }
}
