import Foundation

/// Any JSON value. Control values, `enabled_when` conditions and incoming
/// messages use it so unknown fields and value types pass through untouched.
enum JSONValue: Codable, Equatable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            self = try .object(container.decode([String: JSONValue].self))
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case let .bool(value): try container.encode(value)
        case let .number(value): try container.encode(value)
        case let .string(value): try container.encode(value)
        case let .array(value): try container.encode(value)
        case let .object(value): try container.encode(value)
        }
    }

    static func int(_ value: Int) -> JSONValue {
        .number(Double(value))
    }

    var double: Double? {
        switch self {
        case let .number(value): value
        case let .string(value): Double(value)
        default: nil
        }
    }

    var bool: Bool? {
        switch self {
        case let .bool(value): value
        case let .number(value): value != 0
        default: nil
        }
    }

    var string: String? {
        switch self {
        case let .string(value): value
        case let .number(value): value.rounded() == value ? String(Int(value)) : String(value)
        default: nil
        }
    }

    var object: [String: JSONValue]? {
        if case let .object(value) = self {
            return value
        }
        return nil
    }
}

// MARK: - Descriptors

struct ControlOption: Encodable, Equatable {
    let value: JSONValue
    let label: String
}

enum ControlKind: Equatable {
    case range(min: Double, max: Double, step: Double, scale: String = "linear", unit: String = "")
    case enumeration([ControlOption])
    case bool
    case text
}

struct ControlDescriptor: Encodable, Equatable {
    let id: String
    let label: String
    let group: String
    let kind: String
    var min: Double?
    var max: Double?
    var step: Double?
    var scale: String?
    var unit: String?
    var options: [ControlOption]?
    var enabledWhen: [String: JSONValue]?
    /// Shown, never set (the shutter outside a free-shutter production).
    var readonly: Bool?
    /// Refused while the stream is live (format and codecs).
    var lockedWhileLive: Bool?

    init(id: String, label: String, group: String, kind: ControlKind, enabledWhen: [String: JSONValue]? = nil) {
        self.id = id
        self.label = label
        self.group = group
        self.enabledWhen = enabledWhen
        switch kind {
        case let .range(min, max, step, scale, unit):
            self.kind = "range"
            self.min = min
            self.max = max
            self.step = step
            self.scale = scale
            self.unit = unit
        case let .enumeration(options):
            self.kind = "enum"
            self.options = options
        case .bool:
            self.kind = "bool"
        case .text:
            self.kind = "text"
        }
    }
}

extension ControlDescriptor {
    /// This descriptor, refused while live.
    func lockedWhileLive(_ locked: Bool = true) -> ControlDescriptor {
        var copy = self
        copy.lockedWhileLive = locked ? true : nil
        return copy
    }

    func readonly(_ readonly: Bool) -> ControlDescriptor {
        var copy = self
        copy.readonly = readonly ? true : nil
        return copy
    }
}

struct CommandDescriptor: Encodable, Equatable {
    let name: String
    let label: String
    var args: [String: String]?
}

// MARK: - Device -> server

struct DeviceInfo: Encodable {
    let id: String
    let name: String
    let model: String
    let platform: String
    let osVersion: String
    let appVersion: String
}

struct HelloMessage: Encodable {
    let type = "hello"
    /// 1: with device keys (`public_key`, then `challenge` and `auth`).
    /// 2: production settings, image statistics and the live lock
    /// (docs/SHADING.md).
    let `protocol` = 2
    let device: DeviceInfo
    /// The device key, base64 SEC1 (from the core's `DeviceKeyIdentity`).
    let publicKey: String?
    let controls: [ControlDescriptor]
    let commands: [CommandDescriptor]
}

struct StateMessage: Encodable {
    let type = "state"
    let values: [String: JSONValue]
}

struct StatusMessage: Encodable {
    struct Battery: Encodable {
        let level: Double
        let charging: Bool
    }

    struct Stream: Encodable {
        let state: String
        let url: String
        var bitrateKbps: Int?
        var rttMs: Double?
        var retransmits: Int?
        var dropped: Int?
        var error: String?
        /// The SRT latency the listener agreed to (the larger of both ends).
        var srtLatencyMs: Int?
        /// Per-stage send latency over the last second.
        var latency: LatencyReport?
        /// Frames muxed after their PCR deadline since the stream started.
        var lateVideoFrames: Int?
        var lateAudioFrames: Int?
        /// The encoder's bitrate now, kbit/s (`bitrate_kbps` is measured).
        var targetKbps: Int?
        /// Adaptive bitrate's view of the link: `clear | congested |
        /// overloaded`; omitted when it is off.
        var link: String?
    }

    let type = "status"
    /// Omitted when the level is unknown (simulator).
    let battery: Battery?
    let thermal: String
    let stream: Stream
    let network: String
    /// The phone's clock discipline (`status.clock`), as the core reports it.
    var clock: JSONValue?
    /// Soft genlock, while it is on.
    var genlock: Genlock?

    struct Genlock: Encodable {
        let state: String
        /// Mean phase error against the TAI frame grid over the last second.
        let phaseErrorMs: Double
        let phaseErrorMaxMs: Double
        let correctionPpm: Double
        let frameDurationUs: Double
        /// This model's exposure lead against the reference model, applied to
        /// every video timestamp (`CaptureCalibration`).
        var calibrationUs: Double = Double(CaptureCalibration.exposureLeadNs) / 1000
    }
}

struct PreviewMessage: Encodable {
    let type = "preview"
    let jpeg: String
    let width: Int
    let height: Int
}

struct ResultMessage: Encodable {
    let type = "result"
    let id: JSONValue
    let ok: Bool
    var error: String?
    /// Command output, e.g. `clock_log`.
    var data: JSONValue?
}

/// `image_stats`: the core's luma and colour statistics plus the camera
/// settings at the moment of measurement.
struct ImageStatsMessage: Encodable {
    let type = "image_stats"
    let luma: JSONValue
    var colour: JSONValue?
    var camera: [String: JSONValue]?
}

struct LogMessage: Encodable {
    let type = "log"
    let level: String
    let message: String
}

enum LcpCoding {
    static func encode(_ message: some Encodable) -> String? {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        guard let data = try? encoder.encode(message) else {
            return nil
        }
        return String(data: data, encoding: .utf8)
    }
}

// MARK: - Server -> device

enum LcpIncoming {
    case set(id: JSONValue?, values: [String: JSONValue])
    case command(id: JSONValue?, name: String, args: [String: JSONValue])
    case tally(String)
    case message(String)
    case preview(enabled: Bool, fps: Double, width: Int)
    /// Sign and answer with `auth` (see `DeviceKey`).
    case challenge(server: String, nonce: String)
    /// Production settings (docs/SHADING.md): mains frequency and shutter.
    case production(mainsHz: Int, shutter: ShutterPolicy)
    /// Start or stop `image_stats`.
    case stats(enabled: Bool, fps: Double)

    /// Returns nil for unknown types and malformed messages, which are ignored.
    static func parse(_ text: String) -> LcpIncoming? {
        guard let data = text.data(using: .utf8),
              let json = try? JSONDecoder().decode([String: JSONValue].self, from: data),
              let type = json["type"]?.string
        else {
            return nil
        }
        let id = json["id"].flatMap { $0 == .null ? nil : $0 }
        switch type {
        case "set":
            return json["values"]?.object.map { .set(id: id, values: $0) }
        case "command":
            return json["name"]?.string.map { .command(id: id, name: $0, args: json["args"]?.object ?? [:]) }
        case "tally":
            return json["state"]?.string.map { .tally($0) }
        case "message":
            return json["text"]?.string.map { .message($0) }
        case "challenge":
            guard let server = json["server"]?.string, let nonce = json["nonce"]?.string else {
                return nil
            }
            return .challenge(server: server, nonce: nonce)
        case "production":
            // Compared as a number: Int(Double) traps on huge values.
            guard let hz = json["mains_hz"]?.double, [0, 50, 60].contains(hz) else {
                return nil
            }
            return .production(mainsHz: Int(hz), shutter: ShutterPolicy(json["shutter"]))
        case "stats":
            return .stats(enabled: json["enabled"]?.bool ?? false,
                          fps: json["fps"]?.double.flatMap { $0.isFinite ? min(max($0, 0.2), 4) : nil } ?? 1)
        case "preview":
            return .preview(enabled: json["enabled"]?.bool ?? false,
                            fps: json["fps"]?.double ?? 1,
                            // Clamped before converting: Int(Double) traps on huge values.
                            width: json["width"]?.double.flatMap { $0.isFinite ? Int(min(max($0, 0), 4096)) : nil } ?? 480)
        default:
            return nil
        }
    }
}

/// The production's shutter (docs/SHADING.md, "Shutter").
enum ShutterPolicy: Equatable, Sendable {
    /// One mains period: 1/50 at 50 Hz, 1/60 at 60 Hz.
    case mains
    /// No mains-lit light: the shutter is an exposure control, and auto
    /// shortens it only once the ISO is at its minimum.
    case free
    /// A fixed shutter, 1/x s.
    case fixed(Int)

    /// `"mains"`, `"free"` or a denominator; anything else is `mains`.
    init(_ value: JSONValue?) {
        switch value {
        case .string("free"): self = .free
        case let .number(x) where x.isFinite && x >= 1 && x <= 100_000 && x.rounded() == x: self = .fixed(Int(x))
        default: self = .mains
        }
    }
}
