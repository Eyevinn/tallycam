import Foundation

/// What the phone screen shows of the clock discipline: parsed from the
/// `status.clock` object (see docs/PROTOCOL.md, "status.clock").
struct ClockSummary: Equatable {
    enum Health: String {
        case healthy
        case degraded
        case bad
    }

    var synced = false
    var health: Health = .bad
    var findings: [String] = []
    var estErrorUs: Double?
    var maxErrorUs: Double?
    var reference: String?
    var referenceName: String?
    var stratum: Int?
    var selected: Int?
    var reachable: Int?
    var total: Int?

    init(status: JSONValue) {
        let object = status.object ?? [:]
        synced = object["synced"]?.bool ?? false
        // No health yet (discipline not started): bad unless synced.
        health = object["health"]?.string.flatMap(Health.init(rawValue:)) ?? (synced ? .healthy : .bad)
        if case let .array(items) = object["findings"] {
            findings = items.compactMap(\.string)
        }
        estErrorUs = object["est_error_us"]?.double
        maxErrorUs = object["max_error_us"]?.double
        reference = object["reference"]?.string.flatMap { $0.isEmpty ? nil : $0 }
        referenceName = object["reference_name"]?.string.flatMap { $0.isEmpty ? nil : $0 }
        stratum = object["stratum"]?.double.map { Int($0) }
        selected = object["selected"]?.double.map { Int($0) }
        reachable = object["reachable"]?.double.map { Int($0) }
        total = object["total"]?.double.map { Int($0) }
    }

    /// "212 µs", "3.2 ms", "1.20 s".
    static func formatMicroseconds(_ us: Double?) -> String {
        guard let us, us >= 0 else {
            return "-"
        }
        if us < 1000 {
            return String(format: "%.0f µs", us)
        }
        if us < 1_000_000 {
            return String(format: "%.1f ms", us / 1000)
        }
        return String(format: "%.2f s", us / 1_000_000)
    }
}
