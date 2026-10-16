import Foundation
import Network
import os

private let log = Logger(subsystem: "se.eyevinn.tallycam", category: "clock")

/// The phone's own clock: the Rust core's chrony-like NTP discipline
/// (Strom's chrony.conf: four Ubuntu pools, minsources 3, makestep 1.0 3,
/// TAI leap table) on a thread of its own. The phone's system clock is
/// never touched, and nothing is synced against the control server.
///
/// Frames are stamped through `ClockMapping.at(hostNs:)`, which reads the
/// mapping this service publishes without locks.
final class PhoneClock: @unchecked Sendable {
    static let shared = PhoneClock()

    private let lock = NSLock()
    // Guarded by `lock`.
    private var service: ClockService?
    private var live = false
    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "clock.path")

    /// Starts the discipline once; later calls do nothing.
    func start() {
        lock.lock()
        defer { lock.unlock() }
        guard service == nil else {
            return
        }
        let fm = FileManager.default
        let caches = fm.urls(for: .cachesDirectory, in: .userDomainMask).first
        let support = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
        let options = ClockOptions(mode: .discipline,
                                   chronyConf: nil,
                                   logFile: caches?.appendingPathComponent("clock/clock.log").path,
                                   driftFile: support?.appendingPathComponent("clock/chrony.drift").path)
        do {
            service = try ClockService.start(options: options)
        } catch {
            log.error("Clock discipline did not start: \(error.localizedDescription)")
            return
        }
        log.info("Clock discipline started")
        monitor.pathUpdateHandler = { [weak self] path in
            self?.pathChanged(path)
        }
        monitor.start(queue: queue)
    }

    /// While live the mapping is never stepped and slews at most 500 ppm.
    func setLive(_ live: Bool) {
        lock.lock()
        defer { lock.unlock() }
        guard live != self.live else {
            return
        }
        self.live = live
        service?.setLive(live: live)
    }

    /// `status.clock` for LCP.
    func status() -> JSONValue? {
        let json = lock.withLock { service?.statusJson() }
        guard let json, let data = json.data(using: .utf8) else {
            return nil
        }
        return try? JSONDecoder().decode(JSONValue.self, from: data)
    }

    /// The `clock_log` command: newest log lines and the chronyc-like reports.
    func logReport(kinds: [String], lines: Int) -> JSONValue {
        let (logLines, reports) = lock.withLock {
            (service?.logLines(kinds: kinds, limit: UInt32(clamping: lines)) ?? [],
             service?.reportsText() ?? "")
        }
        return .object([
            "lines": .array(logLines.map { .string($0) }),
            "reports": .string(reports),
        ])
    }

    private func pathChanged(_ path: NWPath) {
        let interfaces = path.availableInterfaces.filter { path.usesInterfaceType($0.type) }
        let key = (interfaces.isEmpty ? path.availableInterfaces : interfaces)
            .map { "\(Self.typeName($0.type)):\($0.name)" }
            .joined(separator: ",")
        let info = NetworkPath(satisfied: path.status == .satisfied,
                               ipv4: path.supportsIPv4,
                               ipv6: path.supportsIPv6,
                               key: key)
        lock.withLock { service }?.pathChanged(path: info)
    }

    private static func typeName(_ type: NWInterface.InterfaceType) -> String {
        switch type {
        case .wifi: "wifi"
        case .cellular: "cellular"
        case .wiredEthernet: "ethernet"
        case .loopback: "loopback"
        case .other: "other"
        @unknown default: "other"
        }
    }
}
