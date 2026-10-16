import Foundation
import Network
import UIKit

/// Device identity for `hello` and the device status for `status`.
@MainActor
final class StatusReporter {
    /// Called with the new thermal state name when it changes.
    var onThermalChange: ((String) -> Void)?

    private let monitor = NWPathMonitor()
    private var network = "none"
    private var thermalObserver: NSObjectProtocol?

    init() {
        UIDevice.current.isBatteryMonitoringEnabled = true
        monitor.pathUpdateHandler = { [weak self] path in
            let name = Self.networkName(path)
            Task { @MainActor in
                self?.network = name
            }
        }
        monitor.start(queue: DispatchQueue(label: "status.network"))
        thermalObserver = NotificationCenter.default.addObserver(
            forName: ProcessInfo.thermalStateDidChangeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else {
                    return
                }
                self.onThermalChange?(Self.thermalName(ProcessInfo.processInfo.thermalState))
            }
        }
    }

    /// The device id: derived from the device key (see `DeviceKey`), so a
    /// server can check it. Stable for this install; nil until the key can be
    /// read (before the first unlock after a reboot).
    static func deviceId() -> String? {
        DeviceKey.current()?.deviceId
    }

    func deviceInfo(key: DeviceKey) -> DeviceInfo {
        let id = key.deviceId
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0"
        return DeviceInfo(id: id,
                          // UIDevice.name is only "iPhone" since iOS 16.
                          name: CameraName.display(custom: CameraName.stored, model: CaptureCalibration.model,
                                                   deviceId: id),
                          model: CaptureCalibration.model,
                          platform: "ios",
                          osVersion: UIDevice.current.systemVersion,
                          appVersion: version)
    }

    func status(stream controller: StreamController) -> StatusMessage {
        let device = UIDevice.current
        let level = device.batteryLevel
        let battery = level < 0 ? nil : StatusMessage.Battery(
            level: (Double(level) * 100).rounded() / 100,
            charging: device.batteryState == .charging || device.batteryState == .full
        )
        // The passphrase stays on the phone (and in the `stream_url` value).
        let url = SrtUrl.redacted(controller.url)
        var stream = StatusMessage.Stream(state: "idle", url: url)
        switch controller.state {
        case .idle:
            if let message = controller.message {
                stream = StatusMessage.Stream(state: "error", url: url, error: message)
            }
        case .connecting:
            // `error`: why a live stream is reconnecting.
            stream = StatusMessage.Stream(state: "connecting", url: url, error: controller.message)
        case .live:
            let stats = controller.stats
            stream = StatusMessage.Stream(state: "live", url: url,
                                          bitrateKbps: stats?.bitrateKbps,
                                          rttMs: stats?.rttMs,
                                          retransmits: stats?.retransmits,
                                          dropped: stats?.dropped,
                                          srtLatencyMs: stats?.srtLatencyMs,
                                          latency: stats?.latency,
                                          lateVideoFrames: stats?.lateVideoFrames,
                                          lateAudioFrames: stats?.lateAudioFrames)
        }
        return StatusMessage(battery: battery,
                             thermal: Self.thermalName(ProcessInfo.processInfo.thermalState),
                             stream: stream,
                             network: network)
    }

    static func thermalName(_ state: ProcessInfo.ThermalState) -> String {
        switch state {
        case .nominal: "nominal"
        case .fair: "fair"
        case .serious: "serious"
        case .critical: "critical"
        @unknown default: "nominal"
        }
    }

    private nonisolated static func networkName(_ path: NWPath) -> String {
        guard path.status == .satisfied else {
            return "none"
        }
        if path.usesInterfaceType(.wifi) {
            return "wifi"
        }
        if path.usesInterfaceType(.cellular) {
            return "cellular"
        }
        if path.usesInterfaceType(.wiredEthernet) {
            return "ethernet"
        }
        return "other"
    }
}
