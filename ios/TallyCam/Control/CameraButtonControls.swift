@preconcurrency import AVFoundation

/// iPhone 16 and later: the Camera Control button offers zoom (the system
/// zoom slider) and exposure, compensation in auto and ISO otherwise, as
/// app controls (docs/UI_DESIGN.md, "Picture"). They set the same values as
/// the rockers, through `CameraControls.apply`. Session queue only.
@available(iOS 18.0, *)
final class CameraButtonControls: NSObject, AVCaptureSessionControlsDelegate, @unchecked Sendable {
    private let session: AVCaptureSession
    private let queue: DispatchQueue
    private unowned let controls: CameraControls
    private var device: AVCaptureDevice?
    /// `exposure_mode` the exposure slider was made for.
    private var exposureMode: String?
    private var exposureSlider: (slider: AVCaptureSlider, range: ClosedRange<Float>)?
    /// The button's overlay is up: the operator drives the sliders, so
    /// their values are not refreshed from the camera.
    private var active = false

    init(session: AVCaptureSession, queue: DispatchQueue, controls: CameraControls) {
        self.session = session
        self.queue = queue
        self.controls = controls
        super.init()
        session.setControlsDelegate(self, queue: queue)
    }

    /// The device or its format changed, or exposure went to or from auto:
    /// new sliders.
    func rebuild(device: AVCaptureDevice?, values: [String: JSONValue]) {
        guard session.supportsControls else {
            return
        }
        self.device = device
        exposureMode = values["exposure_mode"]?.string
        exposureSlider = nil
        session.beginConfiguration()
        defer { session.commitConfiguration() }
        for control in session.controls {
            session.removeControl(control)
        }
        guard let device else {
            return
        }
        var added: [AVCaptureControl] = [AVCaptureSystemZoomSlider(device: device)]
        if let exposure = makeExposureSlider(device) {
            added.append(exposure.slider)
            exposureSlider = exposure
        }
        for control in added where session.canAddControl(control) {
            session.addControl(control)
        }
        refresh(values: values)
    }

    /// The camera's values, about 4 times a second: follows changes made
    /// elsewhere while the button's overlay is not up.
    func refresh(values: [String: JSONValue]) {
        let mode = values["exposure_mode"]?.string
        if (mode == "auto") != (exposureMode == "auto") {
            rebuild(device: device, values: values)
            return
        }
        exposureMode = mode
        guard !active, let (slider, range) = exposureSlider else {
            return
        }
        let key = exposureMode == "auto" ? "ev_bias" : "iso"
        if let value = values[key]?.double, value.isFinite {
            // Only a change: each set may cost a round trip to the overlay.
            let clamped = min(max(Float(value), range.lowerBound), range.upperBound)
            if slider.value != clamped {
                slider.value = clamped
            }
        }
    }

    /// Compensation in auto, ISO otherwise (a locked exposure turns manual,
    /// as with the rocker).
    private func makeExposureSlider(_ device: AVCaptureDevice) -> (slider: AVCaptureSlider, range: ClosedRange<Float>)? {
        // Weak: the session holds the slider and the slider its action.
        weak let controls = controls
        if exposureMode == "auto" {
            let low = device.minExposureTargetBias, high = device.maxExposureTargetBias
            guard high > low else {
                return nil
            }
            let slider = AVCaptureSlider("Exposure", symbolName: "plusminus.circle", in: low ... high, step: 0.1)
            slider.localizedValueFormat = "%@ EV"
            slider.setActionQueue(queue) { value in
                controls?.apply(["ev_bias": .number((Double(value) * 10).rounded() / 10)]) { _, _ in }
            }
            return (slider, low ... high)
        }
        guard device.isExposureModeSupported(.custom) else {
            return nil
        }
        let low = device.activeFormat.minISO, high = device.activeFormat.maxISO
        guard high > low else {
            return nil
        }
        let slider = AVCaptureSlider("ISO", symbolName: "camera.aperture", in: low ... high, step: 1)
        slider.localizedValueFormat = "ISO %@"
        slider.setActionQueue(queue) { [weak self] value in
            var values: [String: JSONValue] = ["iso": .number(Double(value).rounded())]
            if self?.exposureMode != "manual" {
                values["exposure_mode"] = .string("manual")
            }
            controls?.apply(values) { _, _ in }
        }
        return (slider, low ... high)
    }

    // MARK: AVCaptureSessionControlsDelegate

    func sessionControlsDidBecomeActive(_: AVCaptureSession) {
        active = true
    }

    func sessionControlsWillEnterFullscreenAppearance(_: AVCaptureSession) {}

    func sessionControlsWillExitFullscreenAppearance(_: AVCaptureSession) {}

    func sessionControlsDidBecomeInactive(_: AVCaptureSession) {
        active = false
    }
}
