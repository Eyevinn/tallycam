import SwiftUI
#if canImport(TallyCamCoreFFI)
    import TallyCamCoreFFI
#endif

/// Who owns exposure, colour and focus: one chip per group (`EXP AUTO`,
/// `WB LOCKED`, `FOC REMOTE`). Tapping a chip opens the group's controls,
/// read-only while the group is remote (docs/SHADING.md).
struct GroupChips: View {
    @ObservedObject var remote: RemoteControl
    @ObservedObject var shading: Shading
    /// False while the screen is locked: shown, not tappable.
    var interactive = true

    @State private var open: String?

    var body: some View {
        HStack(spacing: 6) {
            ForEach(shading.groups, id: \.group) { view in
                Button {
                    open = view.group
                } label: {
                    GroupChip(view: view)
                }
                .buttonStyle(.plain)
                .disabled(!interactive)
            }
        }
        .sheet(item: Binding(get: { open.map(OpenGroup.init) }, set: { open = $0?.id })) { group in
            GroupSheet(remote: remote, shading: shading, group: group.id)
                .presentationDetents([.medium])
        }
    }
}

private struct OpenGroup: Identifiable {
    let id: String
}

struct GroupChip: View {
    let view: GroupView

    var body: some View {
        HStack(spacing: 4) {
            Text(view.shortName)
                .foregroundStyle(.white.opacity(0.7))
            Text(state)
                .foregroundStyle(color)
        }
        .font(.system(size: 14, weight: .semibold, design: .rounded))
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(.black.opacity(0.55), in: Capsule())
        .overlay(Capsule().strokeBorder(color.opacity(view.indicator == "auto" ? 0 : 0.8), lineWidth: 1))
        .accessibilityLabel("\(view.title) \(state)")
    }

    private var state: String {
        switch view.indicator {
        case "locked": "LOCKED"
        case "remote": "REMOTE"
        case "remote_disconnected": "REMOTE · OFFLINE"
        default: "AUTO"
        }
    }

    private var color: Color {
        switch view.indicator {
        case "locked": .yellow
        case "remote": .cyan
        case "remote_disconnected": .orange
        default: .white
        }
    }
}

/// The controls of one group: Auto, Lock, and the manual values. While the
/// group is remote they are shown read-only; while the hub is away the
/// operator may take the group back (D2).
struct GroupSheet: View {
    @ObservedObject var remote: RemoteControl
    @ObservedObject var shading: Shading
    let group: String

    @State private var confirmTake = false

    var body: some View {
        let view = shading.view(group)
        NavigationStack {
            Form {
                if let view, view.isRemote {
                    Section {
                        Label(view.indicator == "remote"
                            ? "Controlled by \(view.holderLabel ?? "the hub")"
                            : "Controlled by \(view.holderLabel ?? "the hub"), which is not connected",
                            systemImage: "antenna.radiowaves.left.and.right")
                        if view.indicator == "remote_disconnected" {
                            Button("Take local control", role: .destructive) { confirmTake = true }
                        }
                    }
                }
                Section {
                    modePicker
                }
                Section {
                    switch group {
                    case "exposure": exposureControls
                    case "white_balance": colourControls
                    default: focusControls
                    }
                }
                .disabled(view?.isRemote ?? true)
            }
            .navigationTitle(view?.title ?? group)
            .navigationBarTitleDisplayMode(.inline)
            .confirmationDialog("Take \(view?.title.lowercased() ?? group) back from the hub?",
                                isPresented: $confirmTake, titleVisibility: .visible)
            {
                Button("Take local control", role: .destructive) { remote.takeLocal(group: group) }
            } message: {
                Text("The values stay. The hub cannot change them until it takes over again.")
            }
        }
    }

    private var modeKey: String {
        switch group {
        case "exposure": "exposure_mode"
        case "white_balance": "wb_mode"
        default: "focus_mode"
        }
    }

    private var modePicker: some View {
        let options = shading.descriptor(modeKey)?.options ?? []
        let current = shading.values[modeKey]?.string ?? "auto"
        return Picker("Mode", selection: Binding(get: { current }, set: { mode in
            if mode == "auto" {
                remote.localAuto(group: group)
            } else {
                remote.localSet(group: group, values: [modeKey: .string(mode)])
            }
        })) {
            ForEach(options, id: \.label) { option in
                Text(option.label).tag(option.value.string ?? "")
            }
        }
        .pickerStyle(.segmented)
        .disabled(shading.view(group)?.isRemote ?? true)
    }

    @ViewBuilder private var exposureControls: some View {
        slider("iso", label: "ISO", format: "%.0f")
        slider("ev_bias", label: "Compensation", format: "%+.1f EV")
        if let shutter = shading.values["shutter"]?.double {
            LabeledContent("Shutter", value: "1/\(Int(shutter))")
        }
    }

    @ViewBuilder private var colourControls: some View {
        slider("wb_temperature", label: "Temperature", format: "%.0f K")
        slider("wb_tint", label: "Tint", format: "%+.0f")
        Button("White balance once") {
            remote.localSet(group: group, values: [:])
            remote.runLocal(command: "wb_one_shot")
        }
    }

    @ViewBuilder private var focusControls: some View {
        slider("focus_position", label: "Position", format: "%.2f")
    }

    /// A slider for a range control, writable only where its descriptor's
    /// `enabled_when` holds (the matching mode).
    @ViewBuilder private func slider(_ id: String, label: String, format: String) -> some View {
        if let descriptor = shading.descriptor(id), let min = descriptor.min, let max = descriptor.max,
           let value = shading.values[id]?.double
        {
            let enabled = descriptor.readonly != true && (descriptor.enabledWhen ?? [:]).allSatisfy { key, wanted in
                switch wanted {
                case let .array(any): any.contains(shading.values[key] ?? .null)
                default: shading.values[key] == wanted
                }
            }
            let log = descriptor.scale == "log" && min > 0
            VStack(alignment: .leading) {
                LabeledContent(label, value: String(format: format, value))
                Slider(value: Binding(
                    get: { log ? Foundation.log(value) : value },
                    set: { position in
                        let raw = log ? exp(position) : position
                        let step = descriptor.step ?? 0
                        let snapped = step > 0 ? (raw / step).rounded() * step : raw
                        remote.localSet(group: group, values: [id: .number(snapped)])
                    }
                ), in: log ? Foundation.log(min) ... Foundation.log(max) : min ... max)
            }
            .disabled(!enabled)
            .opacity(enabled ? 1 : 0.5)
        }
    }
}
