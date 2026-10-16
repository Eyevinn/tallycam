import AVFoundation

/// The manual exposure values asked for in one `set`. The ISO and shutter
/// controls only record their (clamped) targets here; `CameraControls.apply`
/// then commits both in a single `setExposureModeCustom` call. Two separate
/// calls would each pass the device's current value for the other
/// parameter, and the second would undo the first.
struct ManualExposureRequest: Equatable {
    var iso: Float?
    var duration: CMTime?

    var isEmpty: Bool {
        iso == nil && duration == nil
    }

    /// Arguments for `setExposureModeCustom`. A parameter not in the
    /// request keeps the device's current value.
    func resolved() -> (duration: CMTime, iso: Float) {
        (duration ?? AVCaptureDevice.currentExposureDuration, iso ?? AVCaptureDevice.currentISO)
    }

    /// The `iso` and `shutter` values to report while the request is being
    /// applied, so `state` shows the targets rather than a value from the
    /// middle of the change.
    func stateValues() -> [String: JSONValue] {
        var values: [String: JSONValue] = [:]
        if let iso {
            values["iso"] = .number(Double(iso).rounded())
        }
        if let duration, duration.seconds > 0 {
            values["shutter"] = .int(Int((1 / duration.seconds).rounded()))
        }
        return values
    }
}
