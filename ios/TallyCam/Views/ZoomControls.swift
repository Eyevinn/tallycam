import SwiftUI

/// A vertical zoom rocker: push up to zoom in, down to zoom out; how far
/// sets the speed, for a usable zoom move on air. Letting go stops the
/// zoom where it is.
struct ZoomRocker: View {
    var length = Chrome.rocker
    /// Zoom ramp rate in doublings per second (negative zooms out); 0 stops.
    let onRate: (Double) -> Void

    /// Rate at full deflection.
    private static let maxRate = 2.0

    var body: some View {
        Rocker(high: "T", low: "W", label: "Zoom", length: length) { onRate($0 * Self.maxRate) }
    }
}

/// A spring-loaded vertical rocker (zoom on the left edge, the adjust
/// control on the right): push up or down; how far sets the speed. Letting
/// go springs back and reports 0.
struct Rocker: View {
    let high: String
    let low: String
    /// For VoiceOver.
    let label: String
    var length = Chrome.rocker
    /// Speed from -1 (full down) to 1 (full up). It rises with the square
    /// of the deflection, so small moves are slow; 0 stops.
    let onRate: (Double) -> Void

    private static let steps = 10

    @State private var deflection: CGFloat = 0
    @State private var sentStep = 0

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
            .font(.system(size: 13, weight: .semibold, design: .rounded))
            .foregroundStyle(.white.opacity(0.6))
            .padding(.vertical, 8)
            Circle()
                .fill(.white.opacity(sentStep == 0 ? 0.7 : 0.95))
                .frame(width: 30, height: 30)
                .offset(y: -deflection * travel)
        }
        .frame(width: Chrome.control, height: length)
        .contentShape(Capsule())
        .gesture(
            DragGesture(minimumDistance: 0)
                .onChanged { value in
                    deflection = min(max(-value.translation.height / travel, -1), 1)
                    let step = Int((deflection * CGFloat(Self.steps)).rounded())
                    guard step != sentStep else {
                        return
                    }
                    sentStep = step
                    let share = Double(abs(step)) / Double(Self.steps)
                    onRate(step == 0 ? 0 : (step > 0 ? 1 : -1) * share * share)
                }
                .onEnded { _ in
                    withAnimation(.spring(duration: 0.2)) {
                        deflection = 0
                    }
                    sentStep = 0
                    onRate(0)
                }
        )
        .accessibilityLabel(label)
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
