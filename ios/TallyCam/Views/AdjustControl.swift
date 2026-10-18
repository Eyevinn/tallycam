import SwiftUI

/// One group's controls at the right edge, always there like zoom on the
/// left; the group is chosen by tapping its value in the camera values row
/// (exposure until then, remembered): the group's caption, a mode switch (A
/// over M; tap the other segment, the values are kept), a rocker as long
/// as the zoom rocker that ramps the group's value (up is more; a tap on an
/// end is one step, a double tap on the knob the neutral value, see
/// docs/UI_DESIGN.md "Adjust control"), and the value, which grows while
/// it is being adjusted. Above the rocker where the screen
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
    /// The last tap's value, so quick taps add up before the camera reports.
    @State private var tapped: (id: String, value: Double, at: Date)?
    /// A tap or reset just now: the value label stays large a moment.
    @State private var touched = false
    @State private var touchedTask: Task<Void, Never>?

    var body: some View {
        if shading.groups.contains(group) {
            ViewThatFits(in: .vertical) {
                VStack(spacing: Chrome.gap) {
                    caption
                    modeButton
                    tintSwitch
                    rocker
                    valueLabel
                }
                HStack(spacing: Chrome.gap) {
                    VStack(spacing: Chrome.gap) {
                        caption
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
               length: rockerLength, onStep: nudge, onReset: reset)
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

    /// The group's short name, so the rocker's job is never a guess.
    private var caption: some View {
        Text(group.caption)
            .font(.system(size: 11, weight: .heavy, design: .rounded))
            .foregroundStyle(.white.opacity(0.75))
            .shadow(color: .black.opacity(0.6), radius: 2)
            .fixedSize()
            .allowsHitTesting(false)
    }

    /// Being adjusted: the rocker is held or was just tapped.
    private var adjusting: Bool {
        rampTask != nil || touched
    }

    private var valueLabel: some View {
        Text(valueText)
            .font(.system(size: 13, weight: .semibold, design: .rounded).monospacedDigit())
            .foregroundStyle((isAuto ? Color.white : .yellow).opacity(adjusting ? 1 : 0.8))
            .lineLimit(1)
            .fixedSize()
            .padding(.horizontal, 8)
            .frame(height: Chrome.rowHeight - 6)
            .background(.black.opacity(adjusting ? 0.75 : 0.55), in: Capsule())
            .scaleEffect(adjusting ? 1.25 : 1)
            .animation(.easeOut(duration: 0.15), value: adjusting)
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

    /// What the rocker moves now (docs/UI_DESIGN.md, the adjust control's
    /// table).
    private struct Ramped {
        let id: String
        let label: String
        /// Full speed, in the control's scale per second.
        let speed: Double
        /// A tap on an end, in the control's scale.
        let tapStep: Double
        let log: Bool
        /// The group's mode has to change first (WB and focus need manual).
        let mode: String?
    }

    private var ramped: Ramped? {
        let manual = mode == "manual" ? nil : "manual"
        return switch group {
        case .exposure:
            isAuto ? Ramped(id: "ev_bias", label: "compensation", speed: 2, tapStep: 0.3, log: false, mode: nil)
                : Ramped(id: "iso", label: "ISO", speed: 2, tapStep: 1.0 / 3, log: true, mode: manual)
        case .whiteBalance:
            tint ? Ramped(id: "wb_tint", label: "tint", speed: 60, tapStep: 1, log: false, mode: manual)
                : Ramped(id: "wb_temperature", label: "temperature", speed: 1500, tapStep: 100, log: false,
                         mode: manual)
        case .focus:
            Ramped(id: "focus_position", label: "position", speed: 0.4, tapStep: 0.02, log: false, mode: manual)
        }
    }

    private var highLabel: String {
        switch group {
        case .exposure: "+"
        case .whiteBalance: tint ? "M" : "K+"
        case .focus: "Far"
        }
    }

    private var lowLabel: String {
        switch group {
        case .exposure: "−"
        case .whiteBalance: tint ? "G" : "K−"
        case .focus: "Near"
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

    /// The ramped control's range and step, and its scale (log2 for a log
    /// control).
    private struct Scale {
        let low: Double
        let high: Double
        let step: Double?
        let log: Bool

        func scaled(_ value: Double) -> Double {
            log ? Foundation.log2(value) : value
        }

        func unscaled(_ position: Double) -> Double {
            log ? exp2(position) : position
        }
    }

    private func scale(_ ramped: Ramped) -> Scale? {
        guard let descriptor = shading.descriptor(ramped.id), let low = descriptor.min, let high = descriptor.max,
              high > low, !ramped.log || low > 0
        else {
            return nil
        }
        return Scale(low: low, high: high, step: descriptor.step, log: ramped.log)
    }

    /// One tick: moves the position by the rate and sends it (`move`).
    private func step() {
        guard rate != 0, let ramped, let scale = scale(ramped) else {
            return
        }
        if positionId != ramped.id {
            positionId = ramped.id
            position = nil
            sent = nil
        }
        let current = position ?? scale.scaled(shading.values[ramped.id]?.double ?? (scale.low + scale.high) / 2)
        position = move(to: current + rate * ramped.speed * Self.tick, ramped, scale)
    }

    /// A tap on an end: one step up (1) or down (-1).
    private func nudge(_ direction: Int) {
        guard let ramped, let scale = scale(ramped) else {
            return
        }
        stopRamp()
        let recent = tapped.flatMap { $0.id == ramped.id && Date.now.timeIntervalSince($0.at) < 1 ? $0.value : nil }
        let current = recent ?? shading.values[ramped.id]?.double ?? (scale.low + scale.high) / 2
        move(to: scale.scaled(current) + Double(direction) * ramped.tapStep, ramped, scale)
        if let sent {
            tapped = (ramped.id, sent, .now)
        }
        sent = nil
        touch()
    }

    /// Clamps a position to the range, snaps its value to the step and
    /// sends it if it changed, with a tick (firmer at the end of the range).
    /// Returns the clamped position.
    @discardableResult
    private func move(to position: Double, _ ramped: Ramped, _ scale: Scale) -> Double {
        let next = min(max(position, scale.scaled(scale.low)), scale.scaled(scale.high))
        var value = scale.unscaled(next)
        if let step = scale.step, step > 0 {
            value = (value / step).rounded() * step
        }
        value = min(max(value, scale.low), scale.high)
        guard value != sent else {
            return next
        }
        sent = value
        var values: [String: JSONValue] = [ramped.id: .number(value)]
        if let mode = ramped.mode {
            values[group.modeKey] = .string(mode)
        }
        remote.localSet(values)
        if value <= scale.low || value >= scale.high {
            Haptics.limit()
        } else {
            Haptics.tick()
        }
        return next
    }

    /// A double tap on the knob: the neutral value (docs/UI_DESIGN.md).
    private func reset() {
        stopRamp()
        tapped = nil
        switch group {
        case .exposure:
            remote.localSet(isAuto ? ["ev_bias": .number(0)] : [group.modeKey: .string("auto")])
        case .whiteBalance:
            if tint {
                guard !isAuto else {
                    return
                }
                remote.localSet(["wb_tint": .number(0)])
            } else {
                remote.runLocal(command: "wb_one_shot")
            }
        case .focus:
            remote.localSet([group.modeKey: .string("auto")])
        }
        Haptics.tick()
        touch()
    }

    /// Keeps the value label large for a moment after a tap.
    private func touch() {
        touched = true
        touchedTask?.cancel()
        touchedTask = Task {
            try? await Task.sleep(for: .seconds(0.8))
            if !Task.isCancelled {
                touched = false
            }
        }
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
