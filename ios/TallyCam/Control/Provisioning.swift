import Foundation

/// Provisioning v0 ("scan to connect"): the deep link in the control hub's QR
/// code, `tallycam://connect?control=<ws url>&srt=<srt url>` (both
/// percent-encoded). See docs/PROTOCOL.md, "Provisioning v0".
struct ProvisioningLink: Equatable {
    /// The hub keeps the same value (`DEEP_LINK_SCHEME`), and project.yml
    /// registers it.
    static let scheme = "tallycam"
    static let action = "connect"
    /// Replaced in the SRT URL by `streamId(deviceId:)`.
    static let devicePlaceholder = "{device}"

    let controlUrl: String
    /// Nil when the link has no SRT URL (keep the current one).
    let srtUrl: String?

    /// Host of the control URL, for "Connected to <host>".
    var controlHost: String {
        URL(string: controlUrl)?.host ?? controlUrl
    }

    /// Parses a deep link. Returns nil if it is not a provisioning link or
    /// the URLs in it are not usable (control must be ws/wss, srt must be
    /// srt). `{device}` in the SRT URL becomes this phone's stream id.
    init?(url: URL, deviceId: String) {
        guard url.scheme?.lowercased() == Self.scheme,
              url.host?.lowercased() == Self.action,
              let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems
        else {
            return nil
        }
        func value(_ name: String) -> String? {
            items.first { $0.name == name }?.value?.trimmingCharacters(in: .whitespaces)
        }
        guard let control = value("control"),
              let controlParsed = URL(string: control),
              ["ws", "wss"].contains(controlParsed.scheme?.lowercased()),
              controlParsed.host?.isEmpty == false
        else {
            return nil
        }
        controlUrl = control
        if let srt = value("srt"), !srt.isEmpty {
            let resolved = srt.replacingOccurrences(of: Self.devicePlaceholder,
                                                    with: Self.streamId(deviceId: deviceId))
            guard resolved.lowercased().hasPrefix("srt://") else {
                return nil
            }
            srtUrl = resolved
        } else {
            srtUrl = nil
        }
    }

    /// "cam-" and the first 8 characters of the device id, lowercased.
    static func streamId(deviceId: String) -> String {
        let compact = deviceId.filter { $0.isLetter || $0.isNumber }.lowercased()
        return "cam-" + String(compact.prefix(8))
    }
}
