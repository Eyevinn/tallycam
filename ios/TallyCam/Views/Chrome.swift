import SwiftUI

/// The shooting screen's one grid: every piece of chrome is placed with
/// these, never with its own numbers (docs/APPS.md, "The shooting screen").
enum Chrome {
    /// Width of the tally border plus a margin: chrome stays clear of it.
    static let tallyClearance: CGFloat = 22
    /// Least padding inside a safe-area inset.
    static let minEdge: CGFloat = 8
    /// Round buttons, rockers and the bottom row.
    static let control: CGFloat = 46
    /// The status and camera values rows.
    static let rowHeight: CGFloat = 34
    /// Between the controls of one group.
    static let gap: CGFloat = 10
    /// Between the top rows.
    static let rowGap: CGFloat = 6
    /// Between the zones (top, middle, bottom).
    static let zoneGap: CGFloat = 10
    /// The zoom and adjust rockers' length, where the screen is tall
    /// enough (shorter on a landscape phone, both the same).
    static let rocker: CGFloat = 180

    /// Padding inside the safe area so chrome clears the tally border on
    /// edges without a safe-area inset (landscape top, for example).
    static func edge(_ inset: CGFloat) -> CGFloat {
        max(minEdge, tallyClearance - inset)
    }
}

/// A small round translucent button face, shared by the operator's chrome.
struct ChromeButtonLabel: View {
    let systemName: String
    var active = false
    var size = Chrome.control

    var body: some View {
        Image(systemName: systemName)
            .font(.system(size: size * 0.43, weight: .medium))
            .foregroundStyle(active ? .yellow : .white.opacity(0.85))
            .frame(width: size, height: size)
            .background(.black.opacity(0.35), in: Circle())
            // Visible over the black bars around the picture too.
            .overlay(Circle().strokeBorder(.white.opacity(0.25), lineWidth: 1))
            .contentShape(Circle())
    }
}
