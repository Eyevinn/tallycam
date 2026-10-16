import Foundation

/// The camera's display name: what operators see in the hub and Strom, sent
/// as `hello.device.name`. The device id stays the camera's identity; the
/// name is only a label.
///
/// Since iOS 16 an app cannot read the name the user gave the phone
/// (`UIDevice.name` is just "iPhone"), so the name is set by the operator
/// (`camera_name`) or in Settings, and defaults to the model and the start of
/// the device id: "iPhone 16 · 55dd".
enum CameraName {
    static let defaultsKey = "cameraName"
    static let maxLength = 64

    /// The name set by the operator or in Settings, nil for the default.
    static var stored: String? {
        UserDefaults.standard.string(forKey: defaultsKey).flatMap(normalized)
    }

    /// Trimmed and cut to `maxLength`; nil when empty (back to the default).
    static func normalized(_ name: String) -> String? {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : String(trimmed.prefix(maxLength))
    }

    /// `custom`, or "<model name> · <first 4 of the device id>".
    static func display(custom: String?, model: String, deviceId: String?) -> String {
        if let custom = custom.flatMap(normalized) {
            return custom
        }
        let modelName = marketingName(model) ?? model
        guard let deviceId, !deviceId.isEmpty else {
            return modelName
        }
        return "\(modelName) · \(deviceId.prefix(4).lowercased())"
    }

    /// The marketing name of a hardware model id (`utsname.machine`), for the
    /// models seen so far and their neighbours; nil for others, which then
    /// show their model id.
    static func marketingName(_ model: String) -> String? {
        if model == "arm64" || model == "x86_64" {
            return "Simulator"
        }
        return names[model]
    }

    private static let names: [String: String] = [
        "iPhone11,2": "iPhone XS", "iPhone11,4": "iPhone XS Max", "iPhone11,6": "iPhone XS Max",
        "iPhone11,8": "iPhone XR",
        "iPhone12,1": "iPhone 11", "iPhone12,3": "iPhone 11 Pro", "iPhone12,5": "iPhone 11 Pro Max",
        "iPhone12,8": "iPhone SE",
        "iPhone13,1": "iPhone 12 mini", "iPhone13,2": "iPhone 12", "iPhone13,3": "iPhone 12 Pro",
        "iPhone13,4": "iPhone 12 Pro Max",
        "iPhone14,4": "iPhone 13 mini", "iPhone14,5": "iPhone 13", "iPhone14,2": "iPhone 13 Pro",
        "iPhone14,3": "iPhone 13 Pro Max", "iPhone14,6": "iPhone SE",
        "iPhone14,7": "iPhone 14", "iPhone14,8": "iPhone 14 Plus", "iPhone15,2": "iPhone 14 Pro",
        "iPhone15,3": "iPhone 14 Pro Max",
        "iPhone15,4": "iPhone 15", "iPhone15,5": "iPhone 15 Plus", "iPhone16,1": "iPhone 15 Pro",
        "iPhone16,2": "iPhone 15 Pro Max",
        "iPhone17,3": "iPhone 16", "iPhone17,4": "iPhone 16 Plus", "iPhone17,1": "iPhone 16 Pro",
        "iPhone17,2": "iPhone 16 Pro Max", "iPhone17,5": "iPhone 16e",
        "iPhone18,3": "iPhone 17",
        // iPad generations, short.
        "iPad7,5": "iPad 6", "iPad7,6": "iPad 6", "iPad7,11": "iPad 7", "iPad7,12": "iPad 7",
        "iPad13,18": "iPad 10", "iPad13,19": "iPad 10",
    ]
}
