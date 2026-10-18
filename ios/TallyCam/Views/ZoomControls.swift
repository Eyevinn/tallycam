import SwiftUI
import UIKit

/// A side control running zoom (`SideControl`, docs/UI_DESIGN.md "Side
/// control"): push up to zoom in, down to zoom out; how far sets the speed,
/// for a usable zoom move on air. Letting go stops the zoom where it is. A
/// tap on an end zooms a quarter doubling, a double tap on the knob goes
/// back to the lens's own 1× (no digital zoom). The lens buttons sit under
/// the selector (more than one lens), the zoom factor under the rocker.
struct ZoomControl<Selector: View>: View {
    let controls: CameraControls
    @ObservedObject var shading: Shading
    let side: Side
    /// The value and K / Tint rows beside the block; else above it.
    let rowsBeside: Bool
    @ViewBuilder let selector: () -> Selector

    @State private var ramping = false
    /// The last tap's zoom, so quick taps add up before the camera reports.
    @State private var tapped: (zoom: Double, at: Date)?
    /// A tap or reset just now: the value label stays large a moment.
    @State private var touched = false
    @State private var touchedTask: Task<Void, Never>?

    var body: some View {
        SideLayout(side: side, rowsBeside: rowsBeside, selector: selector) {
            LensSwitch(shading: shading) { lens in
                controls.apply(["lens": .string(lens)]) { _, _ in }
            }
        } rocker: {
            rocker
        } value: {
            SideValueLabel(text: factorText, adjusting: ramping || touched,
                           anchor: side == .left ? .leading : .trailing)
        } rowSwitch: {
            EmptyView()
        }
    }

    private var rocker: some View {
        Rocker(high: "T", low: "W", label: "Zoom", length: SideMetrics.rocker, tint: SideGroup.zoom.color,
               onStep: step, onReset: reset)
        { rate in
            if rate == 0 {
                ramping = false
                controls.stopZoomRamp()
            } else {
                ramping = true
                controls.rampZoom(rate: rate * UiRules.maxZoomRate)
            }
        }
        .accessibilityValue(factorText)
        .onChange(of: zoom) { old, new in
            // The ramp reached the end of the range (the camera reports it
            // to 0.01).
            if ramping, let range, old != new, new <= range.lowerBound + 0.01 || new >= range.upperBound - 0.01 {
                Haptics.limit()
            }
        }
    }

    private var zoom: Double {
        shading.values["zoom"]?.double ?? 1
    }

    private var range: ClosedRange<Double>? {
        guard let descriptor = shading.descriptor("zoom"), let low = descriptor.min, let high = descriptor.max,
              low > 0, high >= low
        else {
            return nil
        }
        return low ... high
    }

    /// The zoom against the wide lens ("2.3×"), as the lens buttons count.
    private var factorText: String {
        let option = shading.descriptor("lens")?.options?.first { $0.value == shading.values["lens"] }
        let factor = (option.flatMap { LensSwitch.factor(label: $0.label) } ?? 1) * zoom
        return UiRules.zoomFactorText(factor)
    }

    /// A tap on an end (`UiRules.tapStep`), from the last tap's zoom when
    /// quick taps add up.
    private func step(_ direction: Int) {
        guard let range else {
            return
        }
        let base = tapped.flatMap { Date.now.timeIntervalSince($0.at) < 1 ? $0.zoom : nil }
        guard let target = UiRules.tapStep(.zoom, shading.values, shading.descriptorMap, tint: false,
                                           direction: direction, base: base)?["zoom"]?.double
        else {
            return
        }
        set(target, atEnd: target <= range.lowerBound || target >= range.upperBound)
    }

    /// A double tap on the knob (`UiRules.neutral`): the lens's own 1×.
    private func reset() {
        if case let .set(values)? = UiRules.neutral(.zoom, shading.values, tint: false),
           let target = values["zoom"]?.double
        {
            set(target, atEnd: false)
        }
    }

    private func set(_ target: Double, atEnd: Bool) {
        tapped = (target, .now)
        controls.apply(["zoom": .number(target)]) { _, _ in }
        if atEnd {
            Haptics.limit()
        } else {
            Haptics.tick()
        }
        touched = true
        touchedTask?.cancel()
        touchedTask = Task {
            try? await Task.sleep(for: .seconds(0.8))
            if !Task.isCancelled {
                touched = false
            }
        }
    }
}

/// The operator's touch feedback: a light tick per step or tap, a firmer
/// one when a value reaches the end of its range.
@MainActor
enum Haptics {
    private static let light = UIImpactFeedbackGenerator(style: .light)
    private static let firm = UIImpactFeedbackGenerator(style: .rigid)

    static func tick() {
        light.impactOccurred()
    }

    static func limit() {
        firm.impactOccurred()
    }
}

/// A spring-loaded vertical rocker (a side control's), the same three
/// gestures on every one: drag up or
/// down, how far sets the speed (letting go springs back and reports 0); tap
/// an end for one step; double tap the knob for the neutral value.
struct Rocker: View {
    let high: String
    let low: String
    /// For VoiceOver.
    let label: String
    let length: CGFloat
    /// The group's colour: the outline and the knob's ring.
    var tint: Color = .white.opacity(0.25)
    /// A tap on an end: 1 up, -1 down.
    var onStep: ((Int) -> Void)?
    /// A double tap on the knob.
    var onReset: (() -> Void)?
    /// Speed from -1 (full down) to 1 (full up). It rises with the square
    /// of the deflection, so small moves are slow; 0 stops.
    let onRate: (Double) -> Void

    private static let steps = 10

    /// A touch shorter and smaller than these is a tap.
    private static let tapTime: TimeInterval = 0.35
    private static let tapSlop: CGFloat = 8
    private static let knob: CGFloat = 40
    /// Full deflection, at most: a tall rocker does not need a longer drag.
    private static let maxTravel: CGFloat = 70

    @State private var deflection: CGFloat = 0
    @State private var sentStep = 0
    @State private var touchStart: Date?
    @State private var lastKnobTap = Date.distantPast
    /// True while a drag is in progress; SwiftUI resets it when the gesture
    /// is cancelled (a call, the app going to the background, the view
    /// replaced), where `onEnded` never runs.
    @GestureState private var dragging = false

    private var travel: CGFloat {
        // Up to the end labels.
        min((length - Self.knob) / 2 - 12, Self.maxTravel)
    }

    var body: some View {
        ZStack {
            Color.clear
                .sideSurface(RoundedRectangle(cornerRadius: 10))
                .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(tint, lineWidth: 1.5))
            VStack {
                Text(high)
                Spacer()
                Text(low)
            }
            .font(.system(size: high.count > 2 || low.count > 2 ? 13 : 15, weight: .semibold, design: .rounded))
            .lineLimit(1)
            .minimumScaleFactor(0.7)
            .foregroundStyle(.white.opacity(0.85))
            .shadow(color: .black.opacity(0.5), radius: 2)
            .padding(.vertical, 10)
            RoundedRectangle(cornerRadius: 8)
                .fill(Color(white: 0.88).opacity(sentStep == 0 ? 0.7 : 0.9))
                .frame(width: Self.knob, height: Self.knob)
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(tint, lineWidth: 2))
                .offset(y: -deflection * travel)
        }
        .frame(width: SideMetrics.rockerWidth, height: length)
        .contentShape(RoundedRectangle(cornerRadius: 10))
        .gesture(
            DragGesture(minimumDistance: 0)
                .updating($dragging) { _, dragging, _ in dragging = true }
                .onChanged { value in
                    if touchStart == nil {
                        touchStart = .now
                    }
                    deflection = min(max(-value.translation.height / travel, -1), 1)
                    // Nothing moves within a tap's slop: a jittery tap is a
                    // tap, never also a ramp.
                    let step = abs(value.translation.height) < Self.tapSlop
                        ? 0 : Int((deflection * CGFloat(Self.steps)).rounded())
                    guard step != sentStep else {
                        return
                    }
                    sentStep = step
                    let share = Double(abs(step)) / Double(Self.steps)
                    onRate(step == 0 ? 0 : (step > 0 ? 1 : -1) * share * share)
                }
                .onEnded { value in
                    let quick = touchStart.map { Date.now.timeIntervalSince($0) < Self.tapTime } ?? false
                    release()
                    if quick, hypot(value.translation.width, value.translation.height) < Self.tapSlop {
                        tap(at: value.startLocation.y)
                    }
                }
        )
        .onChange(of: dragging) { _, dragging in
            // A cancelled drag: stop as if released.
            if !dragging, touchStart != nil {
                release()
            }
        }
        .onDisappear(perform: release)
        .accessibilityElement()
        .accessibilityLabel(label)
        .accessibilityAction(named: "Neutral value") { onReset?() }
        .accessibilityAdjustableAction { direction in
            switch direction {
            case .increment: onStep?(1)
            case .decrement: onStep?(-1)
            @unknown default: break
            }
        }
    }

    /// The knob springs back and the move stops.
    private func release() {
        touchStart = nil
        withAnimation(.spring(duration: 0.2)) {
            deflection = 0
        }
        if sentStep != 0 {
            sentStep = 0
            onRate(0)
        }
    }

    /// Above the knob steps up, below it down; on the knob, a second tap
    /// within the tap time resets.
    private func tap(at y: CGFloat) {
        let fromCentre = y - length / 2
        if abs(fromCentre) <= Self.knob / 2 + 4 {
            if Date.now.timeIntervalSince(lastKnobTap) < Self.tapTime {
                lastKnobTap = .distantPast
                onReset?()
            } else {
                lastKnobTap = .now
            }
        } else {
            lastKnobTap = .distantPast
            onStep?(fromCentre < 0 ? 1 : -1)
        }
    }
}

/// The lens buttons in a zoom side control's mode row: one cell per lens
/// the camera has (from the `lens` control), sharing the row, widest left
/// to longest right, the active one filled. Nothing with a single lens.
struct LensSwitch: View {
    @ObservedObject var shading: Shading
    let onSelect: (String) -> Void

    var body: some View {
        // Widest left, longest right (0.5× 1× 4×): the camera lists them so.
        let options = shading.descriptor("lens")?.options ?? []
        let current = shading.values["lens"]?.string
        if options.count > 1 {
            CellGrid(cells: options.map {
                .init(title: Self.label($0.value.string ?? "", fallback: $0.label), on: $0.value.string == current)
            }, columns: options.count, fontSize: 13) { index in
                if let id = options[index].value.string, id != current {
                    onSelect(id)
                }
            }
        }
    }

    /// The factor at the end of a lens's label ("Tele 5×" is 5), against
    /// the wide lens.
    static func factor(label: String) -> Double? {
        guard let last = label.split(separator: " ").last, last.hasSuffix("×") else {
            return nil
        }
        return Double(last.dropLast())
    }

    private static func label(_ id: String, fallback: String) -> String {
        // The camera puts the factor at the end of its label ("Tele 5×").
        if let factor = fallback.split(separator: " ").last, factor.hasSuffix("×") {
            return String(factor)
        }
        return switch id {
        case "ultra_wide": "0.5×"
        case "wide": "1×"
        case "tele": "TELE"
        default: fallback
        }
    }
}
