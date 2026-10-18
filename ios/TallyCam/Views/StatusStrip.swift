import SwiftUI
import UIKit

/// One compact, low-contrast line of status along the top edge: live state
/// and uptime, bitrate, in the middle the camera's name with its tally
/// (where the screen is wide enough; in landscape with the camera values,
/// one top bar), remote link, clock, battery and (only when it matters)
/// thermal state. The gear opens Settings.
struct StatusStrip<Centre: View>: View {
    @ObservedObject var controller: StreamController
    @ObservedObject var remote: RemoteControl
    @ObservedObject var capture: Capture
    /// In the middle of the line; nil when it has a row of its own.
    var centre: Centre?
    let onSettings: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            TimelineView(.periodic(from: .now, by: 1)) { context in
                HStack(spacing: 12) {
                    liveIndicator(now: context.date)
                        // The stream state never truncates; the middle gives way.
                        .fixedSize()
                    if controller.state == .live, let kbps = controller.stats?.bitrateKbps {
                        // Yellow while adaptive bitrate holds the encoder down.
                        Text(Self.bitrate(kbps))
                            .foregroundStyle(adapting ? .yellow : .white.opacity(0.85))
                    }
                    Spacer(minLength: 8)
                    if let centre {
                        centre
                            .layoutPriority(1)
                        Spacer(minLength: 8)
                    }
                    if capture.orientationChangePending {
                        Image(systemName: "rotate.right")
                            .foregroundStyle(.yellow)
                            .accessibilityLabel("Orientation applies when the stream stops")
                    }
                    linkDot
                    clockDot
                    DeviceIndicators()
                }
            }
            Button(action: onSettings) {
                Image(systemName: "gearshape")
                    .font(.system(size: 17, weight: .regular))
                    .frame(width: 36, height: 30)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("Settings")
        }
        .font(.caption.monospacedDigit().weight(.medium))
        .foregroundStyle(.white.opacity(0.85))
        .lineLimit(1)
        .padding(.leading, 12)
        .padding(.trailing, 2)
        .frame(height: Chrome.rowHeight)
        .sideSurface(Capsule())
    }

    private var adapting: Bool {
        // Both from the same stats, so an operator change in between does
        // not count.
        guard let stats = controller.stats, stats.link != nil,
              let target = stats.targetKbps, let ceiling = stats.ceilingKbps
        else {
            return false
        }
        return target < ceiling
    }

    @ViewBuilder
    private func liveIndicator(now: Date) -> some View {
        switch controller.state {
        case .idle:
            Label {
                Text("STANDBY")
            } icon: {
                Circle().fill(.white.opacity(0.4)).frame(width: 8, height: 8)
            }
            .labelStyle(DotLabelStyle())
        case .connecting:
            Label {
                Text("CONNECTING")
            } icon: {
                Circle().fill(.orange).frame(width: 8, height: 8)
            }
            .labelStyle(DotLabelStyle())
        case .live:
            Label {
                HStack(spacing: 6) {
                    Text("LIVE").fontWeight(.bold)
                    if let since = controller.liveSince {
                        Text(Self.uptime(now.timeIntervalSince(since)))
                    }
                }
            } icon: {
                Circle().fill(.red).frame(width: 8, height: 8)
            }
            .labelStyle(DotLabelStyle())
            .foregroundStyle(.white)
        }
    }

    private var linkDot: some View {
        let (color, label): (Color, String) = switch remote.link {
        case .connected: (.green, "Control hub connected")
        case .connecting: (.orange, "Control hub reconnecting")
        case .notSetUp: (.white.opacity(0.4), "Control hub not set up")
        }
        return Image(systemName: "dot.radiowaves.left.and.right")
            .foregroundStyle(color)
            .accessibilityLabel(label)
    }

    private var clockDot: some View {
        let color: Color = switch remote.clock?.health {
        case .healthy: .green
        case .degraded: .orange
        case .bad, nil: .red
        }
        return Image(systemName: "clock")
            .foregroundStyle(color)
            .accessibilityLabel("Clock \(remote.clock?.health.rawValue ?? "not running")")
    }

    static func uptime(_ seconds: TimeInterval) -> String {
        UiRules.uptime(seconds: seconds)
    }

    static func bitrate(_ kbps: Int) -> String {
        UiRules.bitrate(kbps: kbps)
    }
}

/// Battery level and, when not nominal, the thermal state. Read on each
/// redraw of the strip (once a second).
private struct DeviceIndicators: View {
    var body: some View {
        let device = UIDevice.current
        let level = device.batteryLevel
        let thermal = ProcessInfo.processInfo.thermalState
        HStack(spacing: 8) {
            if let thermalColor = Self.color(thermal) {
                Image(systemName: "thermometer.medium")
                    .foregroundStyle(thermalColor)
                    .accessibilityLabel("Thermal state \(StatusReporter.thermalName(thermal))")
            }
            if level >= 0 {
                let charging = device.batteryState == .charging || device.batteryState == .full
                HStack(spacing: 3) {
                    Image(systemName: charging ? "battery.100.bolt" : Self.batterySymbol(level))
                    Text("\(Int((level * 100).rounded()))%")
                }
                .foregroundStyle(level < 0.2 && !charging ? .red : .white.opacity(0.85))
            }
        }
        .onAppear {
            UIDevice.current.isBatteryMonitoringEnabled = true
        }
    }

    private static func color(_ state: ProcessInfo.ThermalState) -> Color? {
        switch state {
        case .nominal: nil
        case .fair: .yellow
        case .serious: .orange
        case .critical: .red
        @unknown default: nil
        }
    }

    private static func batterySymbol(_ level: Float) -> String {
        switch level {
        case ..<0.13: "battery.0"
        case ..<0.38: "battery.25"
        case ..<0.63: "battery.50"
        case ..<0.88: "battery.75"
        default: "battery.100"
        }
    }
}

/// Small dot then text, tightly spaced.
private struct DotLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 5) {
            configuration.icon
            configuration.title
        }
    }
}
