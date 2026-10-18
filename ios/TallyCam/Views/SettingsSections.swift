import SwiftUI
import UIKit

/// Settings › Picture (docs/UI_DESIGN.md, "Settings"): lens, resolution and
/// frame rate as the active lens offers them, the codecs and the bitrate.
/// All through the control table and the stream controls, as the hub sets
/// them; none of it while live.
struct PictureSection: View {
    let remote: RemoteControl
    @ObservedObject var shading: Shading
    @ObservedObject var controller: StreamController

    /// A tap on the bitrate stepper.
    private static let bitrateStep = 500

    private var live: Bool {
        controller.state != .idle
    }

    var body: some View {
        Section {
            ControlPicker(title: "Lens", id: "lens", remote: remote, shading: shading)
            ControlPicker(title: "Resolution", id: "resolution", remote: remote, shading: shading)
            ControlPicker(title: "Frame rate", id: "frame_rate", remote: remote, shading: shading)
            LabeledPicker(title: "Video codec", selection: Binding(
                get: { controller.videoCodec },
                set: { remote.localStreamSet("video_codec", .string($0.rawValue)) }
            )) {
                ForEach(VideoCodec.allCases, id: \.self) { Text($0.label).tag($0) }
            }
            LabeledPicker(title: "Audio codec", selection: Binding(
                get: { controller.audioCodec },
                set: { remote.localStreamSet("audio_codec", .string($0.rawValue)) }
            )) {
                ForEach(AudioCodec.allCases, id: \.self) { Text($0.label).tag($0) }
            }
            bitrateStepper
        } header: {
            Text("Picture")
        } footer: {
            if live {
                Text("Stop the stream to change.")
            }
        }
        .disabled(live)
    }

    /// 500 kbit/s steps within the format's and codec's range.
    private var bitrateStepper: some View {
        let kbps = controller.bitrateKbps
        let range = controller.bitrateRange
        return Stepper {
            LabeledContent("Bitrate", value: "\(kbps) kbit/s" + (controller.bitrateIsAuto ? " (auto)" : ""))
        } onIncrement: {
            remote.localStreamSet("bitrate_kbps", .int(min(kbps + Self.bitrateStep, range.upperBound)))
        } onDecrement: {
            remote.localStreamSet("bitrate_kbps", .int(max(kbps - Self.bitrateStep, range.lowerBound)))
        }
    }
}

/// Settings › Sync: genlock with its state and offset, and stabilization.
struct SyncSection: View {
    let remote: RemoteControl
    @ObservedObject var shading: Shading
    let capture: Capture

    /// A tap on the offset stepper, µs.
    private static let offsetStep = 100.0

    var body: some View {
        Section {
            if shading.descriptor("genlock") != nil {
                Toggle(isOn: Binding(
                    get: { shading.values["genlock"]?.bool ?? false },
                    set: { remote.localSet(["genlock": .bool($0)]) }
                )) {
                    // The loop's state, refreshed once a second.
                    TimelineView(.periodic(from: .now, by: 1)) { _ in
                        LabeledContent("Genlock", value: genlockState)
                    }
                }
                if shading.values["genlock"]?.bool == true {
                    offsetStepper
                }
            }
            if let options = shading.descriptor("stabilization")?.options, !options.isEmpty {
                ControlPicker(title: "Stabilization", id: "stabilization", remote: remote, shading: shading)
            }
        } header: {
            Text("Sync")
        } footer: {
            Text("Genlock aligns frames with other cameras on the same clock. The offset moves where this camera's "
                + "frames land, for calibration. Stabilization off for multi-camera sync tests.")
        }
    }

    /// Locked (with the phase error), Locking or Off.
    private var genlockState: String {
        guard capture.controls.genlock.isEnabled else {
            return "Off"
        }
        guard let status = capture.controls.genlock.status else {
            return "Locking"
        }
        switch status.state {
        case .locked:
            return UiRules.fmt("Locked, %+.2f ms", status.phaseErrorMs)
        case .acquiring, .freeRun:
            return "Locking"
        }
    }

    /// −50000 to 50000 µs in 100 µs steps; a double tap goes back to 0.
    private var offsetStepper: some View {
        let offset = shading.values["genlock_offset_us"]?.double ?? 0
        let range = GenlockController.offsetRangeUs
        return Stepper {
            LabeledContent("Genlock offset", value: UiRules.fmt("%.0f µs", offset))
                .contentShape(Rectangle())
                .onTapGesture(count: 2) { remote.localSet(["genlock_offset_us": .number(0)]) }
        } onIncrement: {
            remote.localSet(["genlock_offset_us": .number(min(offset + Self.offsetStep, range.upperBound))])
        } onDecrement: {
            remote.localSet(["genlock_offset_us": .number(max(offset - Self.offsetStep, range.lowerBound))])
        }
        .accessibilityHint("Double tap the value for 0")
    }
}

/// Settings › Diagnostics, collapsed: the torch probe, the clock log to
/// share and, in Debug builds, a fixed frame duration.
struct DiagnosticsSection: View {
    @ObservedObject var remote: RemoteControl

    @State private var expanded = false
    @State private var sharedLog: SharedText?
    #if DEBUG
        @State private var frameDurationUs = ""
    #endif

    var body: some View {
        Section {
            DisclosureGroup("Diagnostics", isExpanded: $expanded) {
                Button(remote.torchProbeRunning ? "Torch probe running…" : "Torch probe") {
                    remote.runTorchProbe()
                }
                .disabled(remote.torchProbeRunning)
                if let summary = remote.torchProbeSummary {
                    Text(summary)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }
                Button("Share clock log") {
                    sharedLog = SharedText(text: remote.clockLogText())
                }
                #if DEBUG
                    HStack {
                        TextField("Frame duration, µs", text: $frameDurationUs)
                            .keyboardType(.numberPad)
                        Button("Set") {
                            if let us = Double(frameDurationUs), us.isFinite {
                                remote.runLocal(command: "frame_duration", args: ["us": .number(us)])
                            }
                        }
                        .disabled(Double(frameDurationUs) == nil)
                    }
                #endif
            }
        } footer: {
            Text("For measurements and support. Nothing here is needed to film.")
        }
        .sheet(item: $sharedLog) { shared in
            ActivityView(items: [shared.file ?? shared.text as Any])
        }
    }
}

/// A camera control's choices as a segmented picker, set through the
/// control table (`RemoteControl.localSet`).
private struct ControlPicker: View {
    let title: String
    let id: String
    let remote: RemoteControl
    @ObservedObject var shading: Shading

    var body: some View {
        if let options = shading.descriptor(id)?.options, !options.isEmpty {
            // Tagged by label: the values are JSON.
            LabeledPicker(title: title, selection: Binding(
                get: { options.first { $0.value == shading.values[id] }?.label ?? "" },
                set: { label in
                    if let option = options.first(where: { $0.label == label }) {
                        remote.localSet([id: option.value])
                    }
                }
            )) {
                ForEach(options, id: \.label) { option in
                    Text(Self.short(option.label)).tag(option.label)
                }
            }
        }
    }

    /// Lens labels end in their factor ("Tele 5×"): the factor fits a
    /// segment.
    private static func short(_ label: String) -> String {
        LensSwitch.factor(label: label) != nil ? String(label.split(separator: " ").last ?? "") : label
    }
}

/// A title over a segmented picker, so long option lists keep their room.
private struct LabeledPicker<Value: Hashable, Options: View>: View {
    let title: String
    @Binding var selection: Value
    @ViewBuilder let options: () -> Options

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
            Picker(title, selection: $selection, content: options)
                .pickerStyle(.segmented)
                .labelsHidden()
        }
    }
}

/// Text to share, written to a file so it arrives as one.
private struct SharedText: Identifiable {
    let id = UUID()
    let text: String
    let file: URL?

    init(text: String) {
        self.text = text
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("tallycam-clock-log.json")
        file = (try? text.write(to: url, atomically: true, encoding: .utf8)) == nil ? nil : url
    }
}

/// The system share sheet.
private struct ActivityView: UIViewControllerRepresentable {
    let items: [Any]

    func makeUIViewController(context _: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }

    func updateUIViewController(_: UIActivityViewController, context _: Context) {}
}
