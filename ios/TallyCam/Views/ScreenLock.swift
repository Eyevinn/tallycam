import SwiftUI

/// Under the chrome while the screen is locked: swallows every touch on
/// the picture. The chrome stays visible but takes no touches either, all
/// but the slide to unlock in the bottom row. Remote control, tally and the
/// stream keep running; only touches are blocked (docs/SHADING.md).
struct ScreenLockShield: View {
    var body: some View {
        Color.clear
            .contentShape(Rectangle())
            .onTapGesture {}
            .gesture(DragGesture(minimumDistance: 0))
            .ignoresSafeArea()
    }
}

/// A knob slid all the way to the right unlocks; let go early and it
/// springs back.
struct SlideToUnlock: View {
    let onUnlock: () -> Void

    private static let width: CGFloat = 240
    private static let knob = Chrome.control - 6

    @State private var offset: CGFloat = 0

    var body: some View {
        let travel = Self.width - Self.knob - 6
        ZStack(alignment: .leading) {
            Capsule()
                .fill(.black.opacity(0.5))
            Text("slide to unlock")
                .font(.footnote.weight(.medium))
                .foregroundStyle(.white.opacity(0.6 * (1 - offset / travel)))
                .frame(maxWidth: .infinity)
                .padding(.leading, Self.knob)
            Circle()
                .fill(.white.opacity(0.9))
                .frame(width: Self.knob, height: Self.knob)
                .overlay(Image(systemName: "lock.open.fill").font(.system(size: 18)).foregroundStyle(.black))
                .offset(x: 3 + offset)
                .gesture(
                    DragGesture()
                        .onChanged { value in
                            offset = min(max(value.translation.width, 0), travel)
                        }
                        .onEnded { _ in
                            if offset > travel * 0.85 {
                                onUnlock()
                            }
                            withAnimation(.spring(duration: 0.25)) {
                                offset = 0
                            }
                        }
                )
        }
        .frame(width: Self.width, height: Chrome.control)
        .sensoryFeedback(.impact, trigger: offset >= travel * 0.85)
        .accessibilityElement()
        .accessibilityLabel("Slide to unlock")
        .accessibilityAction(named: "Unlock", onUnlock)
    }
}
