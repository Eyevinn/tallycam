import SwiftUI
import UIKit

/// The zoom rocker on the left edge: push up to zoom in, down to zoom out;
/// how far sets the speed, for a usable zoom move on air. Letting go stops
/// the zoom where it is. A tap on an end zooms a quarter doubling, a double
/// tap on the knob goes back to the lens's own 1× (no digital zoom). The
/// zoom factor shows beside the knob while it moves.
struct ZoomControl: View {
    let controls: CameraControls
    @ObservedObject var shading: Shading
    var length = Chrome.rocker

    /// Rate at full deflection, doublings per second.
    private static let maxRate = 2.0
    /// A tap on an end, in doublings.
    private static let tapStep = 0.25

    @State private var ramping = false
    /// The last tap's zoom, so quick taps add up before the camera reports.
    @State private var tapped: (zoom: Double, at: Date)?
    @State private var badgeTask: Task<Void, Never>?
    @State private var showFactor = false

    var body: some View {
        Rocker(high: "T", low: "W", label: "Zoom", length: length, badge: showFactor ? factorText : nil,
               onStep: step, onReset: reset)
        { rate in
            if rate == 0 {
                ramping = false
                controls.stopZoomRamp()
                hideFactorSoon()
            } else {
                ramping = true
                badgeTask?.cancel()
                showFactor = true
                controls.rampZoom(rate: rate * Self.maxRate)
            }
        }
        .onChange(of: zoom) { old, new in
            // The ramp reached the end of the range.
            // The camera reports it to 0.01.
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
        let factor = (option.flatMap { LensButtons.factor(label: $0.label) } ?? 1) * zoom
        return String(format: factor < 10 ? "%.1f×" : "%.0f×", factor)
    }

    private func step(_ direction: Int) {
        guard let range else {
            return
        }
        let base = tapped.flatMap { Date.now.timeIntervalSince($0.at) < 1 ? $0.zoom : nil } ?? zoom
        let target = min(max(base * exp2(Double(direction) * Self.tapStep), range.lowerBound), range.upperBound)
        set(target, atEnd: target <= range.lowerBound || target >= range.upperBound)
    }

    private func reset() {
        set(1, atEnd: false)
    }

    private func set(_ target: Double, atEnd: Bool) {
        let target = (target * 100).rounded() / 100
        tapped = (target, .now)
        controls.apply(["zoom": .number(target)]) { _, _ in }
        if atEnd {
            Haptics.limit()
        } else {
            Haptics.tick()
        }
        showFactor = true
        hideFactorSoon()
    }

    private func hideFactorSoon() {
        badgeTask?.cancel()
        badgeTask = Task {
            try? await Task.sleep(for: .seconds(1))
            if !Task.isCancelled {
                withAnimation(.easeIn(duration: 0.2)) { showFactor = false }
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

/// A spring-loaded vertical rocker (zoom on the left edge, the adjust
/// control on the right), the same three gestures on every one: drag up or
/// down, how far sets the speed (letting go springs back and reports 0); tap
/// an end for one step; double tap the knob for the neutral value.
struct Rocker: View {
    let high: String
    let low: String
    /// For VoiceOver.
    let label: String
    var length = Chrome.rocker
    /// Shown beside the knob (on the right), the zoom factor while zooming.
    var badge: String?
    /// A tap on an end: 1 up, -1 down.
    var onStep: ((Int) -> Void)?
    /// A double tap on the knob.
    var onReset: (() -> Void)?
    /// Speed from -1 (full down) to 1 (full up). It rises with the square
    /// of the deflection, so small moves are slow; 0 stops.
    let onRate: (Double) -> Void

    private static let steps = 10
    /// Less deflection than this does not move (a tap's jitter).
    private static let deadZone: CGFloat = 0.1
    /// A touch shorter and smaller than these is a tap.
    private static let tapTime: TimeInterval = 0.35
    private static let tapSlop: CGFloat = 8
    private static let knob: CGFloat = 30

    @State private var deflection: CGFloat = 0
    @State private var sentStep = 0
    @State private var touchStart: Date?
    @State private var lastKnobTap = Date.distantPast

    private var travel: CGFloat {
        (length - Chrome.control - 10) / 2
    }

    var body: some View {
        ZStack {
            Capsule()
                .fill(.black.opacity(0.35))
            VStack {
                Text(high)
                Spacer()
                Text(low)
            }
            .font(.system(size: high.count > 2 || low.count > 2 ? 11 : 13, weight: .semibold, design: .rounded))
            .lineLimit(1)
            .minimumScaleFactor(0.7)
            .foregroundStyle(.white.opacity(0.6))
            .padding(.vertical, 8)
            Circle()
                .fill(.white.opacity(sentStep == 0 ? 0.7 : 0.95))
                .frame(width: Self.knob, height: Self.knob)
                .overlay(alignment: .leading) {
                    if let badge {
                        Text(badge)
                            .font(.system(size: 13, weight: .semibold, design: .rounded).monospacedDigit())
                            .foregroundStyle(.white)
                            .padding(.horizontal, 7)
                            .frame(height: 24)
                            .background(.black.opacity(0.55), in: Capsule())
                            .fixedSize()
                            .offset(x: Self.knob + (Chrome.control - Self.knob) / 2 + 6)
                            .transition(.opacity)
                            .allowsHitTesting(false)
                    }
                }
                .offset(y: -deflection * travel)
        }
        .frame(width: Chrome.control, height: length)
        .contentShape(Capsule())
        .gesture(
            DragGesture(minimumDistance: 0)
                .onChanged { value in
                    if touchStart == nil {
                        touchStart = .now
                    }
                    deflection = min(max(-value.translation.height / travel, -1), 1)
                    let step = abs(deflection) < Self.deadZone ? 0 : Int((deflection * CGFloat(Self.steps)).rounded())
                    guard step != sentStep else {
                        return
                    }
                    sentStep = step
                    let share = Double(abs(step)) / Double(Self.steps)
                    onRate(step == 0 ? 0 : (step > 0 ? 1 : -1) * share * share)
                }
                .onEnded { value in
                    let quick = touchStart.map { Date.now.timeIntervalSince($0) < Self.tapTime } ?? false
                    touchStart = nil
                    withAnimation(.spring(duration: 0.2)) {
                        deflection = 0
                    }
                    if sentStep != 0 {
                        sentStep = 0
                        onRate(0)
                    }
                    if quick, hypot(value.translation.width, value.translation.height) < Self.tapSlop {
                        tap(at: value.startLocation.y)
                    }
                }
        )
        .accessibilityElement()
        .accessibilityLabel(label)
        .accessibilityAdjustableAction { direction in
            switch direction {
            case .increment: onStep?(1)
            case .decrement: onStep?(-1)
            @unknown default: break
            }
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

/// One button per lens the camera has (from the `lens` control); the
/// active one is highlighted.
struct LensButtons: View {
    @ObservedObject var shading: Shading
    let onSelect: (String) -> Void

    var body: some View {
        let options = shading.descriptor("lens")?.options ?? []
        let current = shading.values["lens"]?.string
        if options.count > 1 {
            VStack(spacing: 6) {
                // Longest lens on top, like the rocker's T.
                ForEach(options.reversed(), id: \.label) { option in
                    let id = option.value.string ?? ""
                    Button {
                        onSelect(id)
                    } label: {
                        Text(Self.label(id, fallback: option.label))
                            .font(.system(size: 15, weight: .semibold, design: .rounded))
                            .foregroundStyle(id == current ? .yellow : .white.opacity(0.85))
                            .frame(width: Chrome.control, height: Chrome.control)
                            .background(.black.opacity(0.35), in: Circle())
                            .overlay(Circle().strokeBorder(.white.opacity(0.25), lineWidth: 1))
                            .contentShape(Circle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(option.label)
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
