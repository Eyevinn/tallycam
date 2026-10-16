import SwiftUI

/// A vertical zoom rocker: push up to zoom in, down to zoom out; how far
/// sets the speed, for a usable zoom move on air. Letting go stops the
/// zoom where it is.
struct ZoomRocker: View {
    /// Zoom ramp rate in doublings per second (negative zooms out); 0 stops.
    let onRate: (Double) -> Void

    static let height: CGFloat = 180
    private static let travel: CGFloat = 62
    /// Rate at full deflection. Speed rises with the square of the
    /// deflection, so small moves are slow.
    private static let maxRate = 2.0
    private static let steps = 10

    @State private var deflection: CGFloat = 0
    @State private var sentStep = 0

    var body: some View {
        ZStack {
            Capsule()
                .fill(.black.opacity(0.35))
            VStack {
                Text("T")
                Spacer()
                Text("W")
            }
            .font(.system(size: 13, weight: .semibold, design: .rounded))
            .foregroundStyle(.white.opacity(0.6))
            .padding(.vertical, 8)
            Circle()
                .fill(.white.opacity(sentStep == 0 ? 0.7 : 0.95))
                .frame(width: 30, height: 30)
                .offset(y: -deflection * Self.travel)
        }
        .frame(width: 46, height: Self.height)
        .contentShape(Capsule())
        .gesture(
            DragGesture(minimumDistance: 0)
                .onChanged { value in
                    deflection = min(max(-value.translation.height / Self.travel, -1), 1)
                    let step = Int((deflection * CGFloat(Self.steps)).rounded())
                    guard step != sentStep else {
                        return
                    }
                    sentStep = step
                    let share = Double(abs(step)) / Double(Self.steps)
                    onRate(step == 0 ? 0 : (step > 0 ? 1 : -1) * Self.maxRate * share * share)
                }
                .onEnded { _ in
                    withAnimation(.spring(duration: 0.2)) {
                        deflection = 0
                    }
                    sentStep = 0
                    onRate(0)
                }
        )
        .accessibilityLabel("Zoom")
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
                            .frame(width: 46, height: 46)
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
