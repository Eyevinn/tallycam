import Foundation

/// The controls that act on StreamController and Capture rather than on the
/// camera device: the `stream` group and `orientation`.
@MainActor
enum StreamControls {
    static let ids: Set<String> = ["live", "stream_url", "srt_latency_ms", "bitrate_kbps", "video_codec", "audio_codec",
                                   "orientation"]

    /// The bitrate range follows the capture format and codec.
    static func descriptors(_ controller: StreamController) -> [ControlDescriptor] { [
        ControlDescriptor(id: "live", label: "Live", group: "stream", kind: .bool),
        ControlDescriptor(id: "stream_url", label: "Stream URL", group: "stream", kind: .text),
        ControlDescriptor(id: "srt_latency_ms", label: "SRT latency", group: "stream",
                          kind: .range(min: Double(SrtUrl.latencyRange.lowerBound),
                                       max: Double(SrtUrl.latencyRange.upperBound),
                                       step: 10, scale: "log", unit: "ms")),
        ControlDescriptor(id: "bitrate_kbps", label: "Video bitrate", group: "stream",
                          kind: .range(min: Double(controller.bitrateRange.lowerBound),
                                       max: Double(controller.bitrateRange.upperBound),
                                       step: 100, unit: "kbit/s")),
        ControlDescriptor(id: "video_codec", label: "Video codec", group: "stream",
                          kind: .enumeration(VideoCodec.allCases.map {
                              ControlOption(value: .string($0.rawValue), label: $0.label)
                          })),
        ControlDescriptor(id: "audio_codec", label: "Audio codec", group: "stream",
                          kind: .enumeration(AudioCodec.allCases.map {
                              ControlOption(value: .string($0.rawValue), label: $0.label)
                          })),
        ControlDescriptor(id: "orientation", label: "Orientation", group: "image",
                          kind: .enumeration(OrientationMode.allCases.map {
                              ControlOption(value: .string($0.rawValue), label: $0.label)
                          })),
    ] }

    static func values(_ controller: StreamController, capture: Capture?) -> [String: JSONValue] {
        var values: [String: JSONValue] = [
            "live": .bool(controller.state != .idle),
            "stream_url": .string(controller.url),
            "srt_latency_ms": .int(Int((try? SrtUrl(string: controller.url))?.latency ?? SrtUrl.defaultLatency)),
            "bitrate_kbps": .int(controller.bitrateKbps),
            "video_codec": .string(controller.videoCodec.rawValue),
            "audio_codec": .string(controller.audioCodec.rawValue),
        ]
        if let capture {
            values["orientation"] = .string(capture.orientationMode.rawValue)
        }
        return values
    }

    /// Returns an error message, or nil when applied as asked.
    static func apply(_ id: String, _ value: JSONValue, controller: StreamController, capture: Capture) -> String? {
        switch id {
        case "live":
            guard let live = value.bool else {
                return "expected a bool"
            }
            if live {
                controller.goLive(capture: capture)
                return controller.state == .idle ? controller.message ?? "could not start" : nil
            }
            controller.stop()
            return nil
        case "stream_url":
            guard let url = value.string, !url.isEmpty else {
                return "expected a URL"
            }
            controller.url = url
            return controller.state == .idle ? nil : "applies the next time the stream starts"
        case "srt_latency_ms":
            // The URL stays the one place the latency is kept: this sets its
            // `latency` parameter. SRT then uses the larger of both ends.
            guard let ms = value.double else {
                return "expected a number"
            }
            let clamped = min(max(Int(ms.rounded()), SrtUrl.latencyRange.lowerBound), SrtUrl.latencyRange.upperBound)
            guard let url = SrtUrl.settingLatency(clamped, in: controller.url) else {
                return "no SRT URL to set it in"
            }
            controller.url = url
            if controller.state != .idle {
                return "applies the next time the stream starts"
            }
            return clamped == Int(ms.rounded()) ? nil : "clamped to \(clamped)"
        case "bitrate_kbps":
            guard let kbps = value.double else {
                return "expected a number"
            }
            let applied = controller.setBitrate(kbps: Int(kbps.rounded()))
            return applied == Int(kbps.rounded()) ? nil : "clamped to \(applied)"
        case "video_codec":
            guard let codec = value.string.flatMap(VideoCodec.init(rawValue:)) else {
                return "expected " + VideoCodec.allCases.map(\.rawValue).joined(separator: " or ")
            }
            controller.videoCodec = codec
            return controller.state == .idle ? nil : "applies the next time the stream starts"
        case "audio_codec":
            guard let codec = value.string.flatMap(AudioCodec.init(rawValue:)) else {
                return "expected " + AudioCodec.allCases.map(\.rawValue).joined(separator: " or ")
            }
            controller.audioCodec = codec
            return controller.state == .idle ? nil : "applies the next time the stream starts"
        case "orientation":
            guard let mode = value.string.flatMap(OrientationMode.init(rawValue:)) else {
                return "expected auto, landscape or portrait"
            }
            capture.setOrientationMode(mode)
            return capture.orientationChangePending ? "applies the next time the stream starts" : nil
        default:
            return "unknown control"
        }
    }
}
