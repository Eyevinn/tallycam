import SwiftUI

/// The large round Go Live button. A tap starts the stream; stopping takes
/// a deliberate hold (`holdToStop`), with a ring filling up while held, so
/// a stray touch never ends a live stream. Reflects the stream state, also
/// when the control hub starts or stops it.
struct LiveButton: View {
    static let holdToStop = 0.6
    static let size: CGFloat = 76

    let state: StreamController.State
    /// The "Hold to stop" hint above and left of the button (landscape,
    /// bottom-right corner, over the picture) or left of it (portrait).
    var hintAbove = false
    let onStart: () -> Void
    let onStop: () -> Void

    @State private var holding = false
    @State private var showHint = false
    @State private var hintTask: Task<Void, Never>?

    var body: some View {
        gestures(face)
            .overlay(alignment: hintAbove ? .topLeading : .leading) { placedHint }
            .sensoryFeedback(.impact(weight: .heavy), trigger: state, condition: Self.feedback)
            .modifier(accessibility)
    }

    /// Clear of the bottom row and the adjust control: over the picture.
    @ViewBuilder
    private var placedHint: some View {
        if showHint {
            hint
                .alignmentGuide(HorizontalAlignment.leading) { (d: ViewDimensions) -> CGFloat in
                    hintAbove ? d.width : d.width + Chrome.gap
                }
                .alignmentGuide(VerticalAlignment.top) { (d: ViewDimensions) -> CGFloat in d.height + Chrome.gap }
                .transition(.opacity)
        }
    }

    /// A haptic when the stream goes live or stops.
    private static func feedback(old: StreamController.State, new: StreamController.State) -> Bool {
        new == .live || (old != .idle && new == .idle)
    }

    private var face: some View {
        ZStack {
            Circle()
                .fill(fill)
            Circle()
                .strokeBorder(.white.opacity(state == .idle ? 0.9 : 1), lineWidth: 4)
            if holding {
                HoldRing(duration: Self.holdToStop)
            }
            label
        }
        .frame(width: Self.size, height: Self.size)
        .scaleEffect(holding ? 0.94 : 1)
        .animation(.easeOut(duration: 0.15), value: holding)
        .animation(.easeInOut(duration: 0.2), value: state)
        .contentShape(Circle())
    }

    private func gestures(_ content: some View) -> some View {
        content
            .onTapGesture {
                if state == .idle {
                    onStart()
                } else {
                    flashHint()
                }
            }
            .onLongPressGesture(minimumDuration: Self.holdToStop, maximumDistance: 40) {
                guard state != .idle else {
                    return
                }
                holding = false
                onStop()
            } onPressingChanged: { pressing in
                holding = pressing && state != .idle
                if holding {
                    flashHint()
                }
            }
    }

    private var accessibility: LiveButtonAccessibility {
        LiveButtonAccessibility(state: state, onStart: onStart, onStop: onStop)
    }

    @ViewBuilder
    private var label: some View {
        switch state {
        case .idle:
            Text("GO\nLIVE")
                .font(.system(size: 13, weight: .heavy))
                .multilineTextAlignment(.center)
                .foregroundStyle(.white)
        case .connecting:
            ProgressView()
                .tint(.white)
        case .live:
            RoundedRectangle(cornerRadius: 4)
                .fill(.white)
                .frame(width: 22, height: 22)
        }
    }

    private var fill: Color {
        switch state {
        case .idle: .black.opacity(0.35)
        case .connecting: .orange.opacity(0.85)
        case .live: .red
        }
    }

    private var hint: some View {
        Text("Hold to stop")
            .font(.caption.weight(.semibold))
            .foregroundStyle(.white)
            .fixedSize()
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(.black.opacity(0.6), in: Capsule())
            .allowsHitTesting(false)
    }

    private func flashHint() {
        withAnimation { showHint = true }
        hintTask?.cancel()
        hintTask = Task {
            try? await Task.sleep(for: .seconds(2))
            guard !Task.isCancelled else {
                return
            }
            withAnimation { showHint = false }
        }
    }
}

/// Fills up around the button while it is held, over `duration`.
private struct HoldRing: View {
    let duration: Double
    @State private var progress: CGFloat = 0

    var body: some View {
        Circle()
            .trim(from: 0, to: progress)
            .stroke(.white, style: StrokeStyle(lineWidth: 6, lineCap: .round))
            .rotationEffect(.degrees(-90))
            .padding(3)
            .onAppear {
                withAnimation(.linear(duration: duration)) {
                    progress = 1
                }
            }
    }
}

/// VoiceOver: one button; its activation is deliberate enough to stop too.
private struct LiveButtonAccessibility: ViewModifier {
    let state: StreamController.State
    let onStart: () -> Void
    let onStop: () -> Void

    func body(content: Content) -> some View {
        content
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(state == .idle ? "Go live" : "Stop live stream")
            .accessibilityValue(value)
            .accessibilityHint(state == .idle ? "" : "Touch and hold to stop")
            .accessibilityAddTraits(.isButton)
            .accessibilityAction {
                if state == .idle {
                    onStart()
                } else {
                    onStop()
                }
            }
    }

    private var value: String {
        switch state {
        case .idle: "Standby"
        case .connecting: "Connecting"
        case .live: "Live"
        }
    }
}
