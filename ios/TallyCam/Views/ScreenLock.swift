import SwiftUI

/// Over the whole screen while it is locked: swallows every touch except
/// the slide to unlock. Remote control, tally and the stream keep running;
/// only touches are blocked (docs/SHADING.md).
struct ScreenLockLayer: View {
    /// Space below the slider (clear of the live button in portrait).
    let bottomPadding: CGFloat
    let onUnlock: () -> Void

    var body: some View {
        ZStack(alignment: .bottom) {
            Color.clear
                .contentShape(Rectangle())
                .onTapGesture {}
                .gesture(DragGesture(minimumDistance: 0))
                .ignoresSafeArea()
            SlideToUnlock(onUnlock: onUnlock)
                .padding(.bottom, bottomPadding)
        }
    }
}

/// A knob slid all the way to the right unlocks; let go early and it
/// springs back.
private struct SlideToUnlock: View {
    let onUnlock: () -> Void

    private static let width: CGFloat = 240
    private static let knob: CGFloat = 46

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
        .frame(width: Self.width, height: Self.knob + 6)
        .sensoryFeedback(.impact, trigger: offset >= travel * 0.85)
        .accessibilityElement()
        .accessibilityLabel("Slide to unlock")
        .accessibilityAction(named: "Unlock", onUnlock)
    }
}
