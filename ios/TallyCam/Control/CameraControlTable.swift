@preconcurrency import AVFoundation

private let whenManualExposure: [String: JSONValue] = ["exposure_mode": .string("manual")]
private let whenAutoExposure: [String: JSONValue] = ["exposure_mode": .array([.string("auto"), .string("auto_iso")])]
/// The shutter is the operator's in manual and in auto ISO.
private let whenFixedShutter: [String: JSONValue] = ["exposure_mode": .array([.string("manual"), .string("auto_iso")])]
private let whenManualWhiteBalance: [String: JSONValue] = ["wb_mode": .string("manual")]
private let whenManualFocus: [String: JSONValue] = ["focus_mode": .string("manual")]

private let shutterDenominators = [2, 4, 8, 15, 24, 25, 30, 48, 50, 60, 100, 120, 125, 180, 240, 250, 320, 500,
                                   640, 1000, 2000, 4000, 8000]
private let stabilizationModes: [(id: String, label: String, mode: AVCaptureVideoStabilizationMode)] = [
    ("off", "Off", .off),
    ("standard", "Standard", .standard),
    ("cinematic", "Cinematic", .cinematic),
    ("cinematic_extended", "Cinematic extended", .cinematicExtended),
    ("auto", "Auto", .auto),
]
private let temperatureRange: ClosedRange<Float> = 2000 ... 10000
private let tintRange: ClosedRange<Float> = -150 ... 150

private func rounded(_ value: Double, step: Double) -> JSONValue {
    // Divide by the inverse for fractional steps so 0.35 prints as 0.35.
    .number(step >= 1 ? (value / step).rounded() * step : (value / step).rounded() / (1 / step).rounded())
}

private func number(_ value: JSONValue) throws -> Double {
    guard let number = value.double, number.isFinite else {
        throw ControlError("expected a number")
    }
    return number
}

private func choice(_ value: JSONValue, _ ids: [String]) throws -> String {
    guard let id = value.string, ids.contains(id) else {
        throw ControlError("expected one of \(ids.joined(separator: ", "))")
    }
    return id
}

/// Clamps and returns a warning if the value had to change.
private func clamp<T: Comparable & CustomStringConvertible>(_ value: T, _ range: ClosedRange<T>,
                                                            _ warning: inout String?) -> T
{
    let clamped = min(max(value, range.lowerBound), range.upperBound)
    if clamped != value {
        warning = "\(value) outside \(range.lowerBound)...\(range.upperBound), clamped to \(clamped)"
    }
    return clamped
}

private func shutterRange(_ device: AVCaptureDevice) -> ClosedRange<Double> {
    let format = device.activeFormat
    let frame = device.activeVideoMinFrameDuration.seconds
    let longest = min(format.maxExposureDuration.seconds, frame.isFinite && frame > 0 ? frame : 1)
    return format.minExposureDuration.seconds ... max(longest, format.minExposureDuration.seconds)
}

private func clampedGains(_ gains: AVCaptureDevice.WhiteBalanceGains,
                          _ device: AVCaptureDevice) -> AVCaptureDevice.WhiteBalanceGains
{
    let range: ClosedRange<Float> = 1 ... device.maxWhiteBalanceGain
    func clamp(_ value: Float) -> Float {
        value.isFinite ? min(max(value, range.lowerBound), range.upperBound) : range.lowerBound
    }
    return .init(redGain: clamp(gains.redGain), greenGain: clamp(gains.greenGain), blueGain: clamp(gains.blueGain))
}

/// The current gains go out of range while the session is interrupted (in
/// the background, for one), and the conversion throws an Objective-C
/// exception on gains outside 1...maxWhiteBalanceGain, which aborts the app.
private func measuredWhiteBalance(_ device: AVCaptureDevice) -> AVCaptureDevice.WhiteBalanceTemperatureAndTintValues {
    device.temperatureAndTintValues(for: clampedGains(device.deviceWhiteBalanceGains, device))
}

private func lockWhiteBalance(_ values: AVCaptureDevice.WhiteBalanceTemperatureAndTintValues, _ device: AVCaptureDevice) {
    device.setWhiteBalanceModeLocked(with: clampedGains(device.deviceWhiteBalanceGains(for: values), device))
}

extension CameraControls {
    static let table: [CameraControl] = [
        // MARK: Lens

        CameraControl(
            id: "lens", label: "Lens", group: "lens",
            kind: { c in
                c.lenses.isEmpty ? nil : .enumeration(c.lenses.map { ControlOption(value: .string($0.id), label: $0.label) })
            },
            get: { c in c.lenses.first { $0.type == c.device?.deviceType }.map { .string($0.id) } },
            set: { c, value in
                try c.switchLens(to: choice(value, c.lenses.map(\.id)))
                return nil
            },
            reshapes: true
        ),
        // Before frame_rate (its options depend on it) and zoom (a format
        // change resets the zoom).
        CameraControl(
            id: "resolution", label: "Resolution", group: "image",
            kind: { c in
                let resolutions = c.availableResolutions
                return resolutions.isEmpty ? nil : .enumeration(resolutions.map {
                    ControlOption(value: .string($0.rawValue), label: $0.label)
                })
            },
            get: { c in (c.pendingResolution ?? c.activeResolution).map { .string($0.rawValue) } },
            set: { c, value in
                let id = try choice(value, c.availableResolutions.map(\.rawValue))
                guard let resolution = VideoResolution(rawValue: id) else {
                    throw ControlError("unknown resolution \(id)")
                }
                return try c.setResolution(resolution)
            },
            reshapes: true
        ),
        CameraControl(
            id: "frame_rate", label: "Frame rate", group: "image",
            kind: { c in
                let options = c.frameRateOptions
                return options.isEmpty ? nil : .enumeration(options.map {
                    ControlOption(value: .int($0.frameRate), label: "\($0.frameRate) fps" + ($0.binned ? " binned" : ""))
                })
            },
            get: { c in c.currentFrameRate.map { .int($0) } },
            set: { c, value in
                let frameRate = try Int(number(value))
                c.session.beginConfiguration()
                defer { c.session.commitConfiguration() }
                try c.configure { try c.applyFrameRate(frameRate, on: $0) }
                c.revalidateStabilization()
                return nil
            },
            reshapes: true
        ),
        CameraControl(
            // Soft genlock: frame timing locked to the TAI frame grid.
            id: "genlock", label: "Genlock", group: "image",
            kind: { c in c.device == nil ? nil : .bool },
            get: { c in .bool(c.genlock.isEnabled) },
            set: { c, value in
                guard let on = value.bool else {
                    throw ControlError("expected a bool")
                }
                try c.setGenlock(on)
                return nil
            }
        ),
        CameraControl(
            // Calibration: where this model's frames should land after each
            // grid point so its picture matches other models'.
            id: "genlock_offset_us", label: "Genlock offset", group: "image",
            kind: { c in c.device == nil ? nil : .range(min: -50000, max: 50000, step: 10, unit: "us") },
            get: { c in .number(c.genlock.targetOffsetUs) },
            set: { c, value in
                c.genlock.setTargetOffset(us: try number(value))
                return nil
            }
        ),
        CameraControl(
            id: "zoom", label: "Zoom", group: "lens",
            kind: { c in
                c.device.map {
                    .range(min: Double($0.minAvailableVideoZoomFactor),
                           max: min(Double($0.maxAvailableVideoZoomFactor), 10), step: 0.01, scale: "log", unit: "x")
                }
            },
            get: { c in c.device.map { rounded(Double($0.videoZoomFactor), step: 0.01) } },
            set: { c, value in
                let zoom = try number(value)
                return try c.configure { device in
                    var warning: String?
                    let range = Double(device.minAvailableVideoZoomFactor) ... min(Double(device.maxAvailableVideoZoomFactor), 10)
                    device.videoZoomFactor = CGFloat(clamp(zoom, range, &warning))
                    return warning
                }
            }
        ),
        CameraControl(
            id: "stabilization", label: "Stabilization", group: "image",
            kind: { c in
                guard let device = c.device, c.videoOutput.connection(with: .video)?.isVideoStabilizationSupported == true else {
                    return nil
                }
                return .enumeration(stabilizationModes.filter { device.activeFormat.isVideoStabilizationModeSupported($0.mode) }
                    .map { ControlOption(value: .string($0.id), label: $0.label) })
            },
            get: { c in stabilizationModes.first { $0.mode == c.stabilization }.map { .string($0.id) } },
            set: { c, value in
                let id = try choice(value, stabilizationModes.map(\.id))
                guard let mode = stabilizationModes.first(where: { $0.id == id })?.mode,
                      c.device?.activeFormat.isVideoStabilizationModeSupported(mode) == true
                else {
                    throw ControlError("\(id) not supported by the active format")
                }
                c.stabilization = mode
                c.applyConnectionSettings()
                return nil
            }
        ),
        CameraControl(
            id: "torch", label: "Torch", group: "image",
            kind: { c in c.device.flatMap { $0.hasTorch && $0.isTorchModeSupported(.on) ? .bool : nil } },
            get: { c in c.device.map { .bool($0.torchMode == .on) } },
            set: { c, value in
                guard let on = value.bool else {
                    throw ControlError("expected a bool")
                }
                try c.configure { $0.torchMode = on ? .on : .off }
                return nil
            }
        ),

        // MARK: Exposure

        CameraControl(
            id: "exposure_mode", label: "Exposure", group: "exposure",
            kind: { c in
                guard let device = c.device else {
                    return nil
                }
                let modes: [(String, String, AVCaptureDevice.ExposureMode)] = [
                    ("auto", "Auto", .continuousAutoExposure), ("auto_iso", "Auto ISO", .custom),
                    ("locked", "Locked", .locked), ("manual", "Manual", .custom),
                ]
                return .enumeration(modes.filter { device.isExposureModeSupported($0.2) }
                    .map { ControlOption(value: .string($0.0), label: $0.1) })
            },
            get: { c in
                switch c.device?.exposureMode {
                case .custom: .string(c.autoIso ? "auto_iso" : "manual")
                case .locked: .string("locked")
                case nil: nil
                default: .string("auto")
                }
            },
            set: { c, value in
                let id = try choice(value, ["auto", "auto_iso", "locked", "manual"])
                return try c.configure { device in
                    c.autoIso = id == "auto_iso"
                    switch id {
                    case "auto":
                        device.exposureMode = .continuousAutoExposure
                        // iOS auto exposure sets the frame duration itself.
                        return c.genlock.isEnabled ? "genlock needs auto_iso, locked or manual exposure: "
                            + "iOS auto exposure overrides the frame duration" : nil
                    case "locked":
                        device.exposureMode = .locked
                    default:
                        guard device.isExposureModeSupported(.custom) else {
                            throw ControlError("manual exposure not supported")
                        }
                        // Start from what auto exposure picked.
                        if id == "auto_iso" {
                            c.enterAutoIso(device)
                        } else {
                            // The ISO auto exposure reports, not `currentISO`,
                            // which darkens the picture (see `enterAutoIso`).
                            let format = device.activeFormat
                            device.setExposureModeCustom(duration: AVCaptureDevice.currentExposureDuration,
                                                         iso: min(max(device.iso, format.minISO), format.maxISO))
                        }
                    }
                    return nil
                }
            }
        ),
        CameraControl(
            id: "iso", label: "ISO", group: "exposure", enabledWhen: whenManualExposure,
            kind: { c in
                c.device.map { .range(min: Double($0.activeFormat.minISO), max: Double($0.activeFormat.maxISO),
                                      step: 1, scale: "log") }
            },
            get: { c in c.device.map { rounded(Double($0.iso), step: 1) } },
            set: { c, value in
                let iso = try Float(number(value))
                return try c.configure { device in
                    guard device.exposureMode == .custom, !c.autoIso else {
                        throw ControlError("requires exposure_mode manual")
                    }
                    var warning: String?
                    // Committed together with shutter, see ManualExposureRequest.
                    c.exposureRequest.iso = clamp(iso, device.activeFormat.minISO ... device.activeFormat.maxISO, &warning)
                    return warning
                }
            }
        ),
        CameraControl(
            id: "shutter", label: "Shutter", group: "exposure", enabledWhen: whenFixedShutter,
            kind: { c in
                guard let device = c.device else {
                    return nil
                }
                let range = shutterRange(device)
                return .enumeration(shutterDenominators.filter { range.contains(1 / Double($0)) }
                    .map { ControlOption(value: .int($0), label: "1/\($0)") })
            },
            get: { c in
                guard let seconds = c.device?.exposureDuration.seconds, seconds > 0 else {
                    return nil
                }
                return .int(Int((1 / seconds).rounded()))
            },
            set: { c, value in
                let denominator = try number(value)
                guard denominator > 0 else {
                    throw ControlError("expected the denominator of 1/x seconds")
                }
                return try c.configure { device in
                    guard device.exposureMode == .custom else {
                        throw ControlError("requires exposure_mode manual")
                    }
                    var warning: String?
                    let seconds = clamp(1 / denominator, shutterRange(device), &warning)
                    if warning != nil {
                        warning = "1/\(Int(denominator)) out of range, clamped to 1/\(Int((1 / seconds).rounded()))"
                    }
                    // Committed together with ISO, see ManualExposureRequest.
                    c.exposureRequest.duration = CMTime(seconds: seconds, preferredTimescale: 1_000_000)
                    return warning
                }
            }
        ),
        CameraControl(
            id: "ev_bias", label: "Exposure compensation", group: "exposure", enabledWhen: whenAutoExposure,
            kind: { c in
                c.device.map { .range(min: Double($0.minExposureTargetBias), max: Double($0.maxExposureTargetBias),
                                      step: 0.1, unit: "EV") }
            },
            get: { c in
                c.device.map { rounded(Double(c.autoIso ? c.autoIsoBias : $0.exposureTargetBias), step: 0.1) }
            },
            set: { c, value in
                let bias = try Float(number(value))
                return try c.configure { device in
                    if c.autoIso {
                        // Auto ISO aims at the metered level plus this bias.
                        var warning: String?
                        c.autoIsoBias = clamp(bias, device.minExposureTargetBias ... device.maxExposureTargetBias, &warning)
                        return warning
                    }
                    guard device.exposureMode == .continuousAutoExposure || device.exposureMode == .autoExpose else {
                        throw ControlError("requires exposure_mode auto or auto_iso")
                    }
                    var warning: String?
                    device.setExposureTargetBias(clamp(bias, device.minExposureTargetBias ... device.maxExposureTargetBias,
                                                       &warning))
                    return warning
                }
            }
        ),
        CameraControl(
            // Auto ISO rounds the shutter to whole pulses of mains-powered
            // light (1/100, 1/50 at 50 Hz; 1/120, 1/60 at 60 Hz).
            id: "mains_hz", label: "Mains frequency", group: "exposure",
            kind: { c in
                // Off: no mains-lit light in the picture (outdoors), so any
                // shutter is fine and auto ISO may go shorter than 1/100.
                c.device == nil ? nil : .enumeration([ControlOption(value: .int(50), label: "50 Hz"),
                                                      ControlOption(value: .int(60), label: "60 Hz"),
                                                      ControlOption(value: .int(0), label: "Off")])
            },
            get: { c in .int(c.mainsHz) },
            set: { c, value in
                // Compared as a number: Int(Double) traps on large values.
                let hz = try number(value)
                guard hz == 50 || hz == 60 || hz == 0 else {
                    throw ControlError("expected 50, 60 or 0 (off)")
                }
                c.mainsHz = Int(hz)
                return nil
            }
        ),

        // MARK: White balance

        CameraControl(
            id: "wb_mode", label: "White balance", group: "white_balance",
            kind: { c in
                guard let device = c.device else {
                    return nil
                }
                var options: [ControlOption] = []
                if device.isWhiteBalanceModeSupported(.continuousAutoWhiteBalance) {
                    options.append(ControlOption(value: .string("auto"), label: "Auto"))
                }
                if device.isWhiteBalanceModeSupported(.locked) {
                    options.append(ControlOption(value: .string("locked"), label: "Locked"))
                }
                if device.isLockingWhiteBalanceWithCustomDeviceGainsSupported {
                    options.append(ControlOption(value: .string("manual"), label: "Manual"))
                }
                return .enumeration(options)
            },
            get: { c in
                switch c.device?.whiteBalanceMode {
                case .locked: .string(c.whiteBalanceManual ? "manual" : "locked")
                case nil: nil
                default: .string("auto")
                }
            },
            set: { c, value in
                let id = try choice(value, ["auto", "locked", "manual"])
                return try c.configure { device in
                    switch id {
                    case "auto":
                        device.whiteBalanceMode = .continuousAutoWhiteBalance
                    case "locked":
                        device.whiteBalanceMode = .locked
                    default:
                        guard device.isLockingWhiteBalanceWithCustomDeviceGainsSupported else {
                            throw ControlError("manual white balance not supported")
                        }
                        let current = measuredWhiteBalance(device)
                        c.manualWhiteBalance = .init(temperature: min(max(current.temperature, temperatureRange.lowerBound),
                                                                      temperatureRange.upperBound),
                                                     tint: min(max(current.tint, tintRange.lowerBound), tintRange.upperBound))
                        lockWhiteBalance(c.manualWhiteBalance, device)
                    }
                    c.whiteBalanceManual = id == "manual"
                    return nil
                }
            }
        ),
        CameraControl(
            id: "wb_temperature", label: "Temperature", group: "white_balance", enabledWhen: whenManualWhiteBalance,
            kind: { c in
                c.device.map { _ in .range(min: Double(temperatureRange.lowerBound), max: Double(temperatureRange.upperBound),
                                           step: 50, unit: "K") }
            },
            get: { c in c.device.map { rounded(Double(measuredWhiteBalance($0).temperature), step: 50) } },
            set: { c, value in
                let temperature = try Float(number(value))
                return try c.configure { device in
                    guard c.whiteBalanceManual, device.whiteBalanceMode == .locked else {
                        throw ControlError("requires wb_mode manual")
                    }
                    var warning: String?
                    c.manualWhiteBalance.temperature = clamp(temperature, temperatureRange, &warning)
                    lockWhiteBalance(c.manualWhiteBalance, device)
                    return warning
                }
            }
        ),
        CameraControl(
            id: "wb_tint", label: "Tint", group: "white_balance", enabledWhen: whenManualWhiteBalance,
            kind: { c in
                c.device.map { _ in .range(min: Double(tintRange.lowerBound), max: Double(tintRange.upperBound), step: 1) }
            },
            get: { c in c.device.map { rounded(Double(measuredWhiteBalance($0).tint), step: 1) } },
            set: { c, value in
                let tint = try Float(number(value))
                return try c.configure { device in
                    guard c.whiteBalanceManual, device.whiteBalanceMode == .locked else {
                        throw ControlError("requires wb_mode manual")
                    }
                    var warning: String?
                    c.manualWhiteBalance.tint = clamp(tint, tintRange, &warning)
                    lockWhiteBalance(c.manualWhiteBalance, device)
                    return warning
                }
            }
        ),

        // MARK: Focus

        CameraControl(
            id: "focus_mode", label: "Focus", group: "focus",
            kind: { c in
                guard let device = c.device else {
                    return nil
                }
                var options: [ControlOption] = []
                if device.isFocusModeSupported(.continuousAutoFocus) {
                    options.append(ControlOption(value: .string("auto"), label: "Auto"))
                }
                if device.isFocusModeSupported(.locked) {
                    options.append(ControlOption(value: .string("locked"), label: "Locked"))
                }
                if device.isLockingFocusWithCustomLensPositionSupported {
                    options.append(ControlOption(value: .string("manual"), label: "Manual"))
                }
                return options.isEmpty ? nil : .enumeration(options)
            },
            get: { c in
                switch c.device?.focusMode {
                case .locked: .string(c.focusManual ? "manual" : "locked")
                case nil: nil
                default: .string("auto")
                }
            },
            set: { c, value in
                let id = try choice(value, ["auto", "locked", "manual"])
                return try c.configure { device in
                    switch id {
                    case "auto":
                        guard device.isFocusModeSupported(.continuousAutoFocus) else {
                            throw ControlError("auto focus not supported")
                        }
                        device.focusMode = .continuousAutoFocus
                    case "locked":
                        device.focusMode = .locked
                    default:
                        guard device.isLockingFocusWithCustomLensPositionSupported else {
                            throw ControlError("manual focus not supported")
                        }
                        device.setFocusModeLocked(lensPosition: device.lensPosition)
                    }
                    c.focusManual = id == "manual"
                    return nil
                }
            }
        ),
        CameraControl(
            id: "focus_position", label: "Focus position", group: "focus", enabledWhen: whenManualFocus,
            kind: { c in
                c.device.flatMap { $0.isLockingFocusWithCustomLensPositionSupported ? .range(min: 0, max: 1, step: 0.01) : nil }
            },
            get: { c in c.device.map { rounded(Double($0.lensPosition), step: 0.01) } },
            set: { c, value in
                let position = try Float(number(value))
                return try c.configure { device in
                    guard c.focusManual, device.focusMode == .locked else {
                        throw ControlError("requires focus_mode manual")
                    }
                    var warning: String?
                    device.setFocusModeLocked(lensPosition: clamp(position, 0 ... 1, &warning))
                    return warning
                }
            }
        ),
    ]

    static let commands: [CameraCommand] = debugCommands + [
        // Torch against capture timing (docs/GENLOCK.md, "Torch probe"): the
        // phone above a white sheet, manual exposure, a short shutter.
        CameraCommand(
            descriptor: CommandDescriptor(name: "torch_probe", label: "Torch probe (test)",
                                          args: ["mode": "self|emit|watch", "cycles": "5..400",
                                                 "on_ms": "150..2000", "off_ms": "150..2000"]),
            run: { c, args, done in
                var settings = TorchProbe.Settings()
                if let name = args["mode"]?.string {
                    guard let mode = TorchProbe.Mode(rawValue: name) else {
                        done(ControlError("mode is self, emit or watch"))
                        return
                    }
                    settings.mode = mode
                }
                if let cycles = args["cycles"]?.double {
                    // Checked before converting: Int(Double) traps outside Int's range.
                    guard cycles.isFinite, (5 ... 400).contains(cycles) else {
                        done(ControlError("cycles 5..400"))
                        return
                    }
                    settings.cycles = Int(cycles)
                }
                if let on = args["on_ms"]?.double {
                    settings.onMs = on
                }
                if let off = args["off_ms"]?.double {
                    settings.offMs = off
                }
                let report = c.onUpdate
                done(Result {
                    try c.torchProbe.start(controls: c, settings: settings) { level, message in
                        report?(.log(level: level, message: message))
                    }
                }.error)
            }
        ),
        CameraCommand(
            descriptor: CommandDescriptor(name: "focus_at", label: "Focus at point", args: ["x": "0..1", "y": "0..1"]),
            run: { c, args, done in
                guard let x = args["x"]?.double, let y = args["y"]?.double else {
                    done(ControlError("x and y (0..1) required"))
                    return
                }
                let point = c.devicePoint(x: x, y: y)
                done(Result {
                    try c.configure { device in
                        if device.isFocusPointOfInterestSupported, device.isFocusModeSupported(.continuousAutoFocus) {
                            device.focusPointOfInterest = point
                            device.focusMode = .continuousAutoFocus
                            c.focusManual = false
                        }
                        // Manual exposure stays as the operator set it.
                        if device.isExposurePointOfInterestSupported, device.exposureMode != .custom {
                            device.exposurePointOfInterest = point
                            device.exposureMode = .continuousAutoExposure
                        }
                    }
                }.error)
            }
        ),
        CameraCommand(
            descriptor: CommandDescriptor(name: "wb_one_shot", label: "White balance once"),
            run: { c, _, done in
                do {
                    try c.configure { device in
                        guard device.isWhiteBalanceModeSupported(.continuousAutoWhiteBalance) else {
                            throw ControlError("auto white balance not supported")
                        }
                        device.whiteBalanceMode = .continuousAutoWhiteBalance
                        c.whiteBalanceManual = false
                    }
                } catch {
                    done(error)
                    return
                }
                let start = DispatchTime.now()
                func poll() {
                    c.queue.asyncAfter(deadline: .now() + 0.1) {
                        let elapsed = Double(DispatchTime.now().uptimeNanoseconds - start.uptimeNanoseconds) / 1e9
                        let adjusting = c.device?.isAdjustingWhiteBalance ?? false
                        if elapsed < 0.3 || (adjusting && elapsed < 1.5) {
                            poll()
                            return
                        }
                        done(Result { try c.configure { $0.whiteBalanceMode = .locked } }.error)
                    }
                }
                poll()
            }
        ),
    ]
}

private extension Result where Failure == Error {
    var error: Error? {
        if case let .failure(error) = self {
            return error
        }
        return nil
    }
}

extension CameraControls {
    /// Measurement commands, in debug builds only.
    #if DEBUG
        static let debugCommands: [CameraCommand] = [
            // Genlock experiments: a fixed frame duration, on a format with
            // headroom above the frame rate. Refused while genlock runs.
            CameraCommand(
                descriptor: CommandDescriptor(name: "frame_duration", label: "Frame duration (test)",
                                              args: ["us": "1000..1000000"]),
                run: { c, args, done in
                    guard let us = args["us"]?.double, us >= 1000 else {
                        done(ControlError("us (frame duration in microseconds) required"))
                        return
                    }
                    do {
                        done(nil)
                        try c.setTestFrameDuration(us: us)
                    } catch {
                        done(error)
                    }
                }
            ),
        ]
    #else
        static let debugCommands: [CameraCommand] = []
    #endif
}
