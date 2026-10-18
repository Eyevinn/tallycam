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

    /// The control server as `host[:port]`, for the join confirmation.
    var controlServer: String {
        Self.hostPort(controlUrl) ?? controlUrl
    }

    /// "Join <hub host>?" and its message (docs/UI_DESIGN.md, "Joining a
    /// hub"). `live`: the stream URL applies on the next Go Live.
    var joinTitle: String {
        "Join \(controlHost)?"
    }

    func joinMessage(live: Bool) -> String {
        var lines = ["Control hub: \(controlServer)"]
        if let srtUrl {
            lines.append("Stream to: \(Self.hostPort(srtUrl) ?? srtUrl)")
            if live {
                lines.append("The stream URL applies on the next Go Live")
            }
        } else {
            lines.append("Stream: unchanged")
        }
        return lines.joined(separator: "\n")
    }

    /// True when `controlUrl` names the same control server (host and port)
    /// as `current`: such a link applies without asking. False when there
    /// is no current control URL.
    static func sameServer(_ controlUrl: String, _ current: String) -> Bool {
        guard let a = server(of: controlUrl), let b = server(of: current) else {
            return false
        }
        return a == b
    }

    /// `host:port` of a ws/wss URL, the port filled in from the scheme, or
    /// nil when there is no host.
    static func server(of url: String) -> String? {
        guard let parsed = URL(string: url.trimmingCharacters(in: .whitespaces)),
              let host = parsed.host?.lowercased(), !host.isEmpty
        else {
            return nil
        }
        let scheme = parsed.scheme?.lowercased()
        let port = parsed.port ?? (scheme == "wss" ? 443 : scheme == "ws" ? 80 : 0)
        return "\(host):\(port)"
    }

    /// `host` or `host:port` (only an explicit port), for display.
    private static func hostPort(_ url: String) -> String? {
        guard let parsed = URL(string: url), let host = parsed.host, !host.isEmpty else {
            return nil
        }
        return parsed.port.map { "\(host):\($0)" } ?? host
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

    /// The text of a scanned QR code as a TallyCam link, or nil when the
    /// code is something else (a web address, a Wi-Fi code).
    static func scannedLink(_ text: String) -> URL? {
        guard let url = URL(string: text.trimmingCharacters(in: .whitespacesAndNewlines)),
              url.scheme?.lowercased() == scheme
        else {
            return nil
        }
        return url
    }

    /// "cam-" and the first 8 characters of the device id, lowercased.
    static func streamId(deviceId: String) -> String {
        let compact = deviceId.filter { $0.isLetter || $0.isNumber }.lowercased()
        return "cam-" + String(compact.prefix(8))
    }
}
