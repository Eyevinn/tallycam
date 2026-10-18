import SwiftUI

/// One group's controls at the right edge, always there like zoom on the
/// left; the group is chosen by tapping its value in the camera values row
/// (exposure until then, remembered): a mode switch (A over M, or L when
/// locked; tap the other segment, the values are kept), a rocker as long
/// as the zoom rocker that ramps the group's value (up is more), and the
/// value. Above the rocker where the screen
/// is tall enough, else beside it on the inner side. Exposure ramps the
/// bias in auto and ISO otherwise; colour the temperature; focus the lens
/// position (up is far). Moving colour or focus while auto
/// switches them to manual first. A long press on colour's mode button is
/// "white balance once"; a K / Tint switch above colour's rocker picks
/// what it ramps (K each time colour is opened). A locked group (from the
/// hub, or focus locked on the picture) shows M too; a tap on A goes to
/// auto. The operator and the hub may both change the values; the last
/// change wins (docs/SHADING.md).
struct AdjustControl: View {
    let remote: RemoteControl
    @ObservedObject var shading: Shading
    let group: ShadingGroup
    /// The rocker's length, the same as the zoom rocker's.
    var rockerLength = Chrome.rocker

    /// Sets go out at most this often while the rocker is held.
    private static let tick = 0.1

    /// The rocker's speed, -1 ... 1.
    @State private var rate = 0.0
    @State private var rampTask: Task<Void, Never>?
    /// The ramp's own position in the control's scale (log2 for a log
    /// control), unsnapped, while the rocker is held.
    @State private var position: Double?
    @State private var sent: Double?
    /// Colour's rocker ramps the tint instead of the temperature.
    @State private var tint = false
    /// The control the position belongs to (exposure switches between bias
    /// and ISO with its mode).
    @State private var positionId: String?

    var body: some View {
        if shading.groups.contains(group) {
            ViewThatFits(in: .vertical) {
                VStack(spacing: Chrome.gap) {
                    modeButton
                    tintSwitch
                    rocker
                    valueLabel
                }
                HStack(spacing: Chrome.gap) {
                    VStack(spacing: Chrome.gap) {
                        modeButton
                        tintSwitch
                        valueLabel
                    }
                    rocker
                }
            }
            .onDisappear { stopRamp() }
        }
    }

    private var rocker: some View {
        Rocker(high: highLabel, low: lowLabel, label: "\(group.title) \(ramped?.label ?? "")",
               length: rockerLength)
        { newRate in
            rate = newRate
            if newRate == 0 {
                stopRamp()
            } else if rampTask == nil {
                startRamp()
            }
        }
        .accessibilityValue(valueText)
        .alignmentGuide(.rockerCentre) { $0[VerticalAlignment.center] }
    }

    /// K or Tint for colour's rocker, where the camera has a tint.
    @ViewBuilder
    private var tintSwitch: some View {
        if group == .whiteBalance, shading.descriptor("wb_tint") != nil {
            HStack(spacing: 0) {
                switchSegment("K", on: !tint)
                switchSegment("Tint", on: tint)
            }
            .padding(2)
            .background(.black.opacity(0.45), in: Capsule())
            .fixedSize()
        }
    }

    private func switchSegment(_ title: String, on: Bool) -> some View {
        Button {
            if tint != (title == "Tint") {
                stopRamp()
                tint.toggle()
            }
        } label: {
            Text(title)
                .font(.system(size: 12, weight: .bold, design: .rounded))
                .foregroundStyle(on ? .black : .white.opacity(0.8))
                .padding(.horizontal, 8)
                .frame(height: Chrome.rowHeight - 10)
                .background(on ? .white : .clear, in: Capsule())
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
        .accessibilityLabel(title == "K" ? "Rocker moves the temperature" : "Rocker moves the tint")
    }

    private var valueLabel: some View {
        Text(valueText)
            .font(.system(size: 13, weight: .semibold, design: .rounded).monospacedDigit())
            .foregroundStyle(isAuto ? .white : .yellow)
            .lineLimit(1)
            .fixedSize()
            .padding(.horizontal, 8)
            .frame(height: Chrome.rowHeight - 6)
            .background(.black.opacity(0.55), in: Capsule())
            .allowsHitTesting(false)
    }

    // MARK: Mode

    private var mode: String {
        shading.mode(group) ?? "auto"
    }

    private var isAuto: Bool {
        mode == "auto"
    }

    /// Manual where the camera has it, else locked.
    private var heldMode: String {
        let options = shading.descriptor(group.modeKey)?.options?.compactMap(\.value.string) ?? []
        return options.contains("manual") ? "manual" : "locked"
    }

    /// A over M (or L): the filled segment is the mode; a tap on the other
    /// switches, keeping the values. A long press on colour's is "white
    /// balance once".
    private var modeButton: some View {
        VStack(spacing: 0) {
            modeSegment("A", on: isAuto, fill: .white)
            modeSegment(heldLabel, on: !isAuto, fill: .yellow)
        }
        .padding(2)
        .frame(width: Chrome.control)
        .background(.black.opacity(0.45), in: Capsule())
        .overlay(Capsule().strokeBorder(.white.opacity(0.25), lineWidth: 1))
        .onLongPressGesture(minimumDuration: 0.5) {
            if group == .whiteBalance {
                remote.runLocal(command: "wb_one_shot")
            }
        }
        .accessibilityElement()
        .accessibilityAddTraits(.isButton)
        .accessibilityLabel("\(group.title) \(isAuto ? "auto" : mode)")
        .accessibilityHint(isAuto ? "Switches to \(heldMode)" : "Switches to auto")
        .accessibilityAction { toggleMode() }
        .accessibilityActions {
            if group == .whiteBalance {
                Button("White balance once") { remote.runLocal(command: "wb_one_shot") }
            }
        }
    }

    /// M for any held mode: locked and manual both keep the values, and
    /// the rocker turns locked into manual anyway.
    private var heldLabel: String {
        "M"
    }

    private func modeSegment(_ title: String, on: Bool, fill: Color) -> some View {
        Text(title)
            .font(.system(size: 15, weight: .bold, design: .rounded))
            .foregroundStyle(on ? .black : .white.opacity(0.8))
            .frame(maxWidth: .infinity)
            .frame(height: Chrome.rowHeight)
            .background(on ? fill : .clear, in: Capsule())
            .contentShape(Capsule())
            .onTapGesture {
                if !on {
                    toggleMode()
                }
            }
    }

    private func toggleMode() {
        remote.localSet([group.modeKey: .string(isAuto ? heldMode : "auto")])
    }

    // MARK: Ramp

    /// What the rocker moves now.
    private struct Ramped {
        let id: String
        let label: String
        /// Full speed, in the control's scale per second.
        let speed: Double
        let log: Bool
        /// The group's mode has to change first (WB and focus need manual).
        let mode: String?
    }

    private var ramped: Ramped? {
        switch group {
        case .exposure:
            isAuto ? Ramped(id: "ev_bias", label: "compensation", speed: 2, log: false, mode: nil)
                : Ramped(id: "iso", label: "ISO", speed: 2, log: true, mode: mode == "manual" ? nil : "manual")
        case .whiteBalance:
            tint ? Ramped(id: "wb_tint", label: "tint", speed: 60, log: false, mode: mode == "manual" ? nil : "manual")
                : Ramped(id: "wb_temperature", label: "temperature", speed: 1500, log: false,
                         mode: mode == "manual" ? nil : "manual")
        case .focus:
            Ramped(id: "focus_position", label: "position", speed: 0.4, log: false,
                   mode: mode == "manual" ? nil : "manual")
        }
    }

    private var highLabel: String {
        switch group {
        case .exposure: "+"
        case .whiteBalance: tint ? "M" : "K+"
        case .focus: "∞"
        }
    }

    private var lowLabel: String {
        switch group {
        case .exposure: "−"
        case .whiteBalance: tint ? "G" : "K−"
        case .focus: "N"
        }
    }

    private func startRamp() {
        rampTask = Task { @MainActor in
            while !Task.isCancelled {
                step()
                try? await Task.sleep(for: .seconds(Self.tick))
            }
        }
    }

    private func stopRamp() {
        rampTask?.cancel()
        rampTask = nil
        position = nil
        sent = nil
    }

    /// One tick: moves the position by the rate, clamps it to the
    /// descriptor's range, snaps it to its step and sends it if it changed.
    private func step() {
        guard rate != 0, let ramped, let descriptor = shading.descriptor(ramped.id),
              let low = descriptor.min, let high = descriptor.max, high > low,
              !ramped.log || low > 0
        else {
            return
        }
        if positionId != ramped.id {
            positionId = ramped.id
            position = nil
            sent = nil
        }
        let scale: (Double) -> Double = ramped.log ? { Foundation.log2($0) } : { $0 }
        let unscale: (Double) -> Double = ramped.log ? { exp2($0) } : { $0 }
        let current = position ?? scale(shading.values[ramped.id]?.double ?? (low + high) / 2)
        let next = min(max(current + rate * ramped.speed * Self.tick, scale(low)), scale(high))
        position = next
        var value = unscale(next)
        if let step = descriptor.step, step > 0 {
            value = (value / step).rounded() * step
        }
        value = min(max(value, low), high)
        guard value != sent else {
            return
        }
        sent = value
        var values: [String: JSONValue] = [ramped.id: .number(value)]
        if let mode = ramped.mode {
            values[group.modeKey] = .string(mode)
        }
        remote.localSet(values)
    }

    // MARK: Value

    /// What the rocker moves, as it stands.
    private var valueText: String {
        let values = shading.values
        switch group {
        case .exposure:
            if isAuto {
                return String(format: "%+.1f EV", values["ev_bias"]?.double ?? 0)
            }
            return values["iso"]?.double.map { String(format: "ISO %.0f", $0) } ?? "ISO"
        case .whiteBalance:
            if tint {
                return values["wb_tint"]?.double.map { String(format: "Tint %+.0f", $0) } ?? "Tint"
            }
            return values["wb_temperature"]?.double.map { String(format: "%.0f K", $0) } ?? "K"
        case .focus:
            if isAuto {
                return "AF"
            }
            return values["focus_position"]?.double.map { String(format: "F %.2f", $0) } ?? "F"
        }
    }
}
