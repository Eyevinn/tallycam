import SwiftUI

/// Everything that is not needed while shooting: connection, stream URL,
/// orientation, clock and device details.
struct SettingsView: View {
    @ObservedObject var remote: RemoteControl
    @ObservedObject var controller: StreamController
    @ObservedObject var capture: Capture
    @Environment(\.dismiss) private var dismiss

    @State private var confirmForget = false
    /// The control URL when the sheet opened, to reconnect on close if edited.
    @State private var initialControlUrl = ""

    var body: some View {
        NavigationStack {
            Form {
                cameraSection
                connectionSection
                streamSection
                orientationSection
                clockSection
                aboutSection
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .onAppear { initialControlUrl = remote.url }
        .onDisappear {
            if remote.url != initialControlUrl {
                remote.reconnect()
            }
        }
    }

    // MARK: Sections

    private var cameraSection: some View {
        Section {
            TextField(CameraName.display(custom: nil, model: CaptureCalibration.model,
                                         deviceId: DeviceKey.current()?.deviceId),
                      text: $controller.cameraName)
                .submitLabel(.done)
                .autocorrectionDisabled()
        } header: {
            Text("Camera name")
        } footer: {
            Text("How the hub and Strom show this camera. The operator can rename it too; empty uses the model.")
        }
    }

    private var connectionSection: some View {
        Section {
            HStack {
                Circle()
                    .fill(linkColor)
                    .frame(width: 9, height: 9)
                Text(linkText)
                Spacer()
                if let host = remote.host {
                    Text(host)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            urlField("ws://host:8090/lcp/v0/device", text: $remote.url)
                .onSubmit {
                    initialControlUrl = remote.url
                    remote.reconnect()
                }
            if !remote.isUnconfigured {
                Button("Disconnect and forget server", role: .destructive) {
                    confirmForget = true
                }
                .confirmationDialog("Forget the control hub?", isPresented: $confirmForget,
                                    titleVisibility: .visible)
                {
                    Button("Forget server", role: .destructive) {
                        remote.forget()
                        initialControlUrl = remote.url
                    }
                } message: {
                    Text("The phone disconnects from the hub. Scan its QR code to connect again.")
                }
            }
        } header: {
            Text("Control hub")
        } footer: {
            Label("To connect, scan the QR code in the control hub with the iPhone Camera app.",
                  systemImage: "qrcode.viewfinder")
        }
    }

    private var streamSection: some View {
        Section {
            urlField("srt://host:port?streamid=...", text: $controller.url)
                .disabled(controller.state != .idle)
            LabeledContent("Video", value: "\(controller.videoCodec.label), \(controller.bitrateKbps) kbit/s"
                + (controller.bitrateIsAuto ? " (auto)" : ""))
            Toggle("Adaptive bitrate", isOn: $controller.adaptiveBitrate)
            if let stats = controller.stats, controller.state == .live {
                if controller.adaptiveBitrate, let target = stats.targetKbps {
                    LabeledContent("Encoder now", value: "\(target) kbit/s")
                }
                if let rtt = stats.rttMs {
                    LabeledContent("Round trip", value: String(format: "%.0f ms", rtt))
                }
                if let retransmits = stats.retransmits, let dropped = stats.dropped {
                    LabeledContent("Retransmitted / dropped", value: "\(retransmits) / \(dropped)")
                }
            }
        } header: {
            Text("Stream (SRT)")
        } footer: {
            Text((controller.state == .idle
                ? "Set by the QR code. Edit only if you know the address."
                : "Stop the stream to change the address.")
                + " Adaptive bitrate lowers the video bitrate when the network cannot carry it, and raises it again up to the bitrate set.")
        }
    }

    private var orientationSection: some View {
        Section {
            Picker("Orientation", selection: Binding(get: { capture.orientationMode },
                                                     set: { capture.setOrientationMode($0) }))
            {
                ForEach(OrientationMode.allCases, id: \.self) { mode in
                    Label(mode.label, systemImage: mode.symbol).tag(mode)
                }
            }
            .pickerStyle(.segmented)
            LabeledContent("Picture", value: "\(capture.status)")
        } header: {
            Text("Orientation")
        } footer: {
            if capture.orientationChangePending {
                Text("The stream keeps its geometry while live; the new setting applies when it stops.")
            } else {
                Text("Auto follows how the phone is held.")
            }
        }
    }

    private var clockSection: some View {
        Section {
            if let clock = remote.clock {
                LabeledContent("Health") {
                    HStack(spacing: 6) {
                        Circle().fill(Self.color(clock.health)).frame(width: 9, height: 9)
                        Text(clock.health.rawValue.capitalized)
                    }
                }
                LabeledContent("Synchronised", value: clock.synced ? "Yes" : "No")
                LabeledContent("Estimated error", value: ClockSummary.formatMicroseconds(clock.estErrorUs))
                LabeledContent("Maximum error", value: ClockSummary.formatMicroseconds(clock.maxErrorUs))
                LabeledContent("Reference", value: reference(clock))
                if let stratum = clock.stratum, stratum > 0 {
                    LabeledContent("Stratum", value: "\(stratum)")
                }
                if let reachable = clock.reachable, let total = clock.total {
                    LabeledContent("Sources reachable", value: "\(reachable) of \(total)")
                }
                ForEach(clock.findings, id: \.self) { finding in
                    Text(finding)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            } else {
                Text("Clock not running")
                    .foregroundStyle(.secondary)
            }
        } header: {
            Text("Clock")
        } footer: {
            Text("The phone's own NTP-disciplined clock, used to timestamp frames.")
        }
    }

    private var aboutSection: some View {
        let key = DeviceKey.current()
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "?"
        return Section("About") {
            if let key, let info = remote.deviceInfo(key: key) {
                LabeledContent("App version", value: "\(info.appVersion) (\(build))")
                LabeledContent("Camera name", value: info.name)
                LabeledContent("Model", value: info.model)
                LabeledContent("iOS", value: info.osVersion)
                LabeledContent("Stream id", value: ProvisioningLink.streamId(deviceId: info.id))
                // The server's camera page shows the same start of the fingerprint.
                LabeledContent("Device key", value: DeviceKey.shortFingerprint(key.identity.fingerprint))
                    .font(.callout.monospaced())
            } else {
                LabeledContent("Device key", value: "waiting for unlock")
            }
        }
    }

    // MARK: Helpers

    private func urlField(_ prompt: String, text: Binding<String>) -> some View {
        TextField(prompt, text: text)
            .font(.callout.monospaced())
            .keyboardType(.URL)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .submitLabel(.done)
    }

    private var linkColor: Color {
        switch remote.link {
        case .connected: .green
        case .connecting: .orange
        case .notSetUp: .gray
        }
    }

    private var linkText: String {
        switch remote.link {
        case .connected: "Connected"
        case .connecting: "Reconnecting"
        case .notSetUp: "Not set up"
        }
    }

    private func reference(_ clock: ClockSummary) -> String {
        switch (clock.referenceName, clock.reference) {
        case let (name?, address?): "\(name) (\(address))"
        case let (name?, nil): name
        case let (nil, address?): address
        case (nil, nil): "-"
        }
    }

    private static func color(_ health: ClockSummary.Health) -> Color {
        switch health {
        case .healthy: .green
        case .degraded: .orange
        case .bad: .red
        }
    }
}
