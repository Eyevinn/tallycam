import SwiftUI

/// The shooting screen's one grid: every piece of chrome is placed with
/// these, never with its own numbers (docs/UI_DESIGN.md, "Look").
enum Chrome {
    /// The doc's minimum edge (docs/UI_DESIGN.md, "Look"): the least
    /// padding inside a safe-area inset, see `edge`.
    static let minEdge: CGFloat = 8
    /// Width of the tally border plus a margin: chrome stays clear of it.
    static let tallyClearance: CGFloat = 22
    /// Round buttons and the bottom row.
    static let control: CGFloat = 46
    /// The status and camera values rows.
    static let rowHeight: CGFloat = 34
    /// Between the controls of one group.
    static let gap: CGFloat = 10
    /// Between the top rows.
    static let rowGap: CGFloat = 6
    /// Between the zones (top, middle, bottom).
    static let zoneGap: CGFloat = 10

    /// Padding inside the safe area: chrome sits max(inset + minEdge, tally
    /// clearance) from each screen edge, 8 pt clear of the Dynamic Island
    /// and the home indicator, 22 pt where an edge has no inset (as Android).
    static func edge(_ inset: CGFloat) -> CGFloat {
        max(minEdge, tallyClearance - inset)
    }
}

extension View {
    /// The chrome's see-through surface: black at 15 % with a thin white
    /// border, no blur, so the picture shows through (docs/UI_DESIGN.md,
    /// "See-through, no blur").
    func sideSurface(_ shape: some InsettableShape, dim: Double = 0.15, visible: Bool = true) -> some View {
        background {
            if visible {
                shape
                    .fill(.black.opacity(dim))
                    .overlay(shape.strokeBorder(.white.opacity(0.4), lineWidth: 1))
            }
        }
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
            .shadow(color: .black.opacity(0.5), radius: 2)
            // Its border keeps it visible over the black bars too.
            .sideSurface(Circle())
            .contentShape(Circle())
    }
}
