import SwiftUI

/// A side control running exposure, colour or focus (`SideControl`,
/// docs/UI_DESIGN.md "Side control"): the rocker ramps the group's value
/// (up is more; a tap on an end is one step, a double tap on the knob the
/// neutral value); the mode switch under the selector (A over M; tap the
/// other segment, the values are kept); the value under the rocker, which
/// grows while it is being adjusted. Exposure ramps the bias in auto and
/// ISO otherwise; colour the temperature; focus the lens position (up is
/// far). Moving colour or focus while auto switches them to manual first.
/// A long press on colour's mode switch is "white balance once"; a K / Tint
/// switch under colour's value picks what it ramps (K each time colour is
/// chosen). A locked group (from the hub, or focus locked on the picture)
/// shows M too; a tap on A goes to auto. The operator and the hub may both
/// change the values; the last change wins (docs/SHADING.md).
struct AdjustControl<Selector: View>: View {
    let remote: RemoteControl
    @ObservedObject var shading: Shading
    /// The group the rocker adjusts.
    let group: ShadingGroup
    let side: Side
    /// The value and K / Tint rows beside the block; else above it.
    let rowsBeside: Bool
    @ViewBuilder let selector: () -> Selector

    /// Sets go out at most this often while the rocker is held.
    private static var tick: Double { 0.1 }

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
        SideLayout(side: side, rowsBeside: rowsBeside, selector: selector) {
            modeButton
        } rocker: {
            rocker
        } value: {
            valueLabel
        } rowSwitch: {
            tintSwitch
        }
        .onDisappear { stopRamp() }
    }

    private var rocker: some View {
        Rocker(high: highLabel, low: lowLabel, label: "\(group.title) \(ramped?.label ?? "")",
               length: SideMetrics.rocker, tint: SideGroup(group).color, onStep: nudge, onReset: reset)
        { newRate in
            rate = newRate
            if newRate == 0 {
                stopRamp()
            } else if rampTask == nil {
                startRamp()
            }
        }
        .accessibilityValue(valueText)
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
            .sideSurface(RoundedRectangle(cornerRadius: 10))
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
                .foregroundStyle(on ? .black : .white)
                .shadow(color: .black.opacity(on ? 0 : 0.5), radius: 2)
                .padding(.horizontal, 8)
                .frame(height: SideMetrics.rowHeight - 4)
                .background(on ? .white.opacity(0.7) : .clear, in: RoundedRectangle(cornerRadius: 8))
                .contentShape(RoundedRectangle(cornerRadius: 8))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
        .accessibilityLabel(title == "K" ? "Rocker moves the temperature" : "Rocker moves the tint")
    }

    /// Being adjusted: the rocker is held or was just tapped.
    private var adjusting: Bool {
        rampTask != nil || touched
    }

    private var valueLabel: some View {
        SideValueLabel(text: valueText, tone: isAuto ? .white : .yellow, adjusting: adjusting,
                       anchor: side == .left ? .leading : .trailing)
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

    /// A | M: the filled cell is the mode; a tap on the other switches,
    /// keeping the values. M for any held mode: locked and manual both keep
    /// the values, and the rocker turns locked into manual anyway. A long
    /// press on colour's is "white balance once".
    private var modeButton: some View {
        // White balance has a third cell, ◎: white balance once from the
        // picture, then held (M).
        let once = group == .whiteBalance
        let cells: [CellGrid.Cell] = [.init(title: "A", on: isAuto, fill: .white),
                                      .init(title: "M", on: !isAuto, fill: .yellow)]
            + (once ? [.init(title: "◎", on: false)] : [])
        return CellGrid(cells: cells, columns: cells.count, fontSize: 17) { index in
            if index == 2 {
                remote.runLocal(command: "wb_one_shot")
                Haptics.tick()
            } else if (index == 0) != isAuto {
                toggleMode()
            }
        }
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

    private func toggleMode() {
        remote.localSet([group.modeKey: .string(isAuto ? heldMode : "auto")])
    }

    // MARK: Ramp

    /// What the rocker moves now (`UiRules.ramped`).
    private var ramped: Ramped? {
        UiRules.ramped(group, shading.values, shading.descriptorMap, tint: tint)
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

    /// A tap on an end: one step up (1) or down (-1) (`UiRules.tapStep`),
    /// from the last tap's value when quick taps add up.
    private func nudge(_ direction: Int) {
        guard let ramped else {
            return
        }
        stopRamp()
        let recent = tapped.flatMap { $0.id == ramped.id && Date.now.timeIntervalSince($0.at) < 1 ? $0.value : nil }
        guard let set = UiRules.tapStep(SideGroup(group), shading.values, shading.descriptorMap, tint: tint,
                                        direction: direction, base: recent),
            let value = set[ramped.id]?.double
        else {
            return
        }
        tapped = (ramped.id, value, .now)
        remote.localSet(set)
        let descriptor = shading.descriptor(ramped.id)
        if value <= descriptor?.min ?? -.infinity || value >= descriptor?.max ?? .infinity {
            Haptics.limit()
        } else {
            Haptics.tick()
        }
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
        switch UiRules.neutral(SideGroup(group), shading.values, tint: tint) {
        case let .set(values):
            remote.localSet(values)
        case let .command(name):
            remote.runLocal(command: name)
        case nil:
            return
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
        UiRules.adjustValueText(group, shading.values, tint: tint)
    }
}
