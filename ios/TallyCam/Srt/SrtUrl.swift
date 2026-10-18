import Foundation

/// A parsed `srt://host:port?streamid=&latency=&passphrase=&pbkeylen=`
/// caller URL. The parsing is the core's (`srtCallerOptionsFromUrl`), so the
/// app accepts exactly what its SRT caller can do.
struct SrtUrl: Equatable {
    /// The caller options with the core's defaults (5 s timeouts, 50 Mbit/s
    /// pacing, no reconnect); LiveStream sets what it needs on top.
    let options: SrtCallerOptions

    static let defaultLatency: UInt16 = 120
    /// Range of the `srt_latency_ms` control.
    static let latencyRange = 0 ... 8000

    struct ParseError: LocalizedError, Equatable {
        let message: String

        var errorDescription: String? {
            message
        }
    }

    init(string: String) throws {
        do {
            options = try srtCallerOptionsFromUrl(url: string)
        } catch let SrtError.Config(message) {
            throw ParseError(message: message)
        }
    }

    var host: String {
        options.host
    }

    var port: UInt16 {
        options.port
    }

    var streamId: String? {
        options.streamId
    }

    /// Receiver and sender latency in milliseconds.
    var latency: UInt16 {
        options.latencyMs
    }

    /// `url` with the passphrase's value and a stream id's media token
    /// (`t=`) replaced by `***`, for logs and status. Other strings are
    /// returned as they are.
    static func redacted(_ url: String) -> String {
        // Over the whole text, as Android: URLComponents would stop at a
        // `#` in the passphrase and leave the rest of it in the log.
        var text = url.replacingOccurrences(of: "(?i)passphrase=[^&]*", with: "passphrase=***",
                                            options: .regularExpression)
        // The media token in a structured stream id (`#!::r=…,t=…`), plain
        // or percent-encoded.
        text = text.replacingOccurrences(of: "(?i)([?&,:]t=)[^,&]*", with: "$1***", options: .regularExpression)
        return text.replacingOccurrences(of: "(?i)((?:%2C|%3A)t%3D)(?:(?!%2C)[^&])*", with: "$1***",
                                         options: .regularExpression)
    }

    /// `url` with its `latency` query parameter set to `ms` (added when
    /// missing). The other parameters are kept as written, so an escaped
    /// passphrase stays escaped. Nil when `url` is not an SRT URL.
    static func settingLatency(_ ms: Int, in url: String) -> String? {
        guard var components = URLComponents(string: url.trimmingCharacters(in: .whitespacesAndNewlines)),
              components.scheme?.lowercased() == "srt"
        else {
            return nil
        }
        var items = components.percentEncodedQueryItems ?? []
        let value = String(ms)
        if let index = items.firstIndex(where: { $0.name.lowercased() == "latency" }) {
            items[index].value = value
        } else {
            items.append(URLQueryItem(name: "latency", value: value))
        }
        components.percentEncodedQueryItems = items
        return components.string
    }
}
