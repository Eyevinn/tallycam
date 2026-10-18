import SwiftUI

extension SideGroup {
    /// Marks the control only (rocker, chosen segment, the values row's
    /// underline), never a value's text (docs/UI_DESIGN.md, "Look").
    var color: Color {
        switch self {
        case .zoom: Color(red: 0x3B / 255, green: 0x82 / 255, blue: 0xF6 / 255)
        case .exposure: Color(red: 0xEF / 255, green: 0x44 / 255, blue: 0x44 / 255)
        case .whiteBalance: Color(red: 0xA8 / 255, green: 0x55 / 255, blue: 0xF7 / 255)
        case .focus: Color(red: 0x22 / 255, green: 0xC5 / 255, blue: 0x5E / 255)
        }
    }
}

/// The sizes both side controls share, the same on every screen and for
/// every group, so that nothing moves when a group changes
/// (docs/UI_DESIGN.md, "Side control", "Sizes"): cells easy to hit without
/// looking, a rocker of a fixed, moderate size as tall as the selector and
/// mode row beside it.
enum SideMetrics {
    /// A selector or mode cell, never smaller.
    static let cellHeight: CGFloat = 44
    static let cellWidth: CGFloat = 46
    /// Around and between the cells.
    static let cellGap: CGFloat = 2
    /// The selector and the mode row.
    static let blockWidth = 2 * cellWidth + 3 * cellGap
    static let selectorHeight = 2 * cellHeight + 3 * cellGap
    static let modeHeight = cellHeight + 2 * cellGap
    static let blockHeight = selectorHeight + Chrome.gap + modeHeight
    static let rockerWidth: CGFloat = 56
    static let rocker = blockHeight
    /// The value label's and the K / Tint switch's rows.
    static let rowHeight = Chrome.rowHeight - 6
    static let rowsHeight = 2 * rowHeight + Chrome.gap
    /// The value and K / Tint column beside the block.
    static let besideWidth: CGFloat = 104

    /// The rocker and the block, without the value column.
    static let coreWidth = blockWidth + Chrome.gap + rockerWidth
    /// With the value column beside the block.
    static let fullWidth = coreWidth + Chrome.gap + besideWidth
}

/// One side control (docs/UI_DESIGN.md, "Side control"): the rocker at the
/// screen edge and, on its inner side, the selector over the mode switch or
/// lens buttons; the value and, for white balance, the K / Tint switch
/// beside the block on its inner side (above it where the screen is too
/// narrow for that, a portrait phone). Runs zoom (`ZoomControl`) or a
/// shading group (`AdjustControl`). On a phone it sits in its bottom corner;
/// on an iPad its centre sits at 65 % of the screen's height, where the
/// thumbs rest, moved up only as far as it must to clear the bottom row
/// (the bottom of its zone); the same on both sides.
struct SideControl: View {
    let side: Side
    /// Both sides' choices: choosing the other side's group swaps the two,
    /// the chosen one again hides this side.
    @Binding var sides: Sides
    let remote: RemoteControl
    let controls: CameraControls
    /// Not observed here: only the children that show values observe it, so
    /// the 4 Hz camera values do not rebuild the whole side control.
    let shading: Shading
    /// The value and K / Tint rows beside the block; else above it.
    let rowsBeside: Bool
    /// The whole screen's height, safe areas included.
    let screenHeight: CGFloat
    /// Phones: in the bottom corner, against the bottom edge.
    var inCorner = false

    /// Where thumbs rest, from the top of the screen.
    private static let restShare: CGFloat = 0.65

    /// The zone this side control has (between the top rows and the bottom
    /// row), in screen coordinates.
    @State private var zone: CGRect = .zero

    /// The control's height, hidden or not: the rocker with the tab above
    /// it, or with the value rows (and the tab beside them) above.
    private var height: CGFloat {
        rowsBeside ? SideMetrics.rocker + Chrome.gap + Chrome.control
            : SideMetrics.rocker + Chrome.gap + SideMetrics.rowsHeight
    }

    /// From the top of the zone to the control's top.
    private var top: CGFloat {
        let wanted = inCorner ? .infinity : Self.restShare * screenHeight - zone.minY - height / 2
        return max(0, min(wanted, zone.height - height))
    }

    private var group: SideGroup {
        sides.group(side) ?? (side == .left ? .zoom : .exposure)
    }

    var body: some View {
        // The tab at a fixed place just above the rocker at the screen edge,
        // shown or hidden alike: the control folds away around it.
        Group {
            if rowsBeside {
                VStack(alignment: side == .left ? .leading : .trailing, spacing: Chrome.gap) {
                    tab
                        .frame(width: SideMetrics.rockerWidth)
                    folding(control)
                }
            } else {
                // The rows above the block leave the corner above the rocker.
                folding(control)
                    .overlay(alignment: side == .left ? .topLeading : .topTrailing) {
                        tab
                            .frame(width: SideMetrics.rockerWidth)
                            .padding(.top, SideMetrics.rowsHeight - Chrome.control)
                    }
            }
        }
        .offset(y: top)
        .frame(maxHeight: .infinity, alignment: .top)
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { zone = $0 }
    }

    /// Hidden: the control keeps its place (nothing moves) but is not
    /// drawn and takes no touches.
    private func folding(_ control: some View) -> some View {
        control
            .opacity(sides.hidden(side) ? 0 : 1)
            .allowsHitTesting(!sides.hidden(side))
    }

    /// Hide and show (docs/UI_DESIGN.md, "Hide and show: one tab"): centred
    /// over the rocker, in the group's colour; the chevron points out to the edge while the control
    /// shows (a tap folds it away), inward while hidden (a tap brings it
    /// back with its group).
    private var tab: some View {
        let hidden = sides.hidden(side)
        let pointsLeft = (side == .left) != hidden
        return Button {
            sides = hidden ? sides.show(side) : sides.hide(side)
        } label: {
            Image(systemName: pointsLeft ? "chevron.left" : "chevron.right")
                .font(.system(size: 17, weight: .bold))
                .foregroundStyle(.white)
                .shadow(color: .black.opacity(0.5), radius: 2)
                .frame(width: Chrome.control, height: Chrome.control)
                .background(group.color.opacity(0.85), in: RoundedRectangle(cornerRadius: 10))
                .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.white.opacity(0.4), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(hidden ? "Show" : "Hide") the \(side == .left ? "left" : "right") control (\(group.title))")
    }

    private var control: some View {
        Group {
            if let shadingGroup = group.shading {
                AdjustControl(remote: remote, shading: shading, group: shadingGroup, side: side,
                              rowsBeside: rowsBeside)
                {
                    selector
                }
            } else {
                ZoomControl(controls: controls, shading: shading, side: side, rowsBeside: rowsBeside) {
                    selector
                }
            }
        }
        // A new group starts with fresh state (the K / Tint switch on K).
        .id(group)
        .frame(width: rowsBeside ? SideMetrics.fullWidth : SideMetrics.coreWidth)
    }

    private var selector: some View {
        CellGrid(cells: SideGroup.allCases.map {
            .init(title: $0.caption, on: $0 == group, fill: $0.color, text: .white)
        }, columns: 2, fontSize: 13)
        { index in
            sides = sides.choose(side, SideGroup.allCases[index])
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(side == .left ? "Left" : "Right") control: \(group.title)")
    }
}

/// The skeleton both kinds of side control share, mirrored on the left:
/// the rocker at the edge, the block (selector over mode row) beside it,
/// both bottom-aligned and the same height; the value over the K / Tint row
/// beside the block on its inner side, level with the mode row, or above
/// the block. Every slot has a fixed size; what does not fit its slot (a
/// wide value) overflows it without moving anything.
struct SideLayout<Selector: View, Under: View, Bar: View, Value: View, Switch: View>: View {
    let side: Side
    let rowsBeside: Bool
    @ViewBuilder let selector: () -> Selector
    @ViewBuilder let under: () -> Under
    @ViewBuilder let rocker: () -> Bar
    @ViewBuilder let value: () -> Value
    /// The K / Tint switch's row, kept (empty) for every group.
    @ViewBuilder let rowSwitch: () -> Switch

    private var inward: HorizontalAlignment {
        side == .left ? .leading : .trailing
    }

    var body: some View {
        if rowsBeside {
            HStack(alignment: .bottom, spacing: Chrome.gap) {
                if side == .left {
                    bar
                    block
                    rows
                } else {
                    rows
                    block
                    bar
                }
            }
        } else {
            VStack(alignment: inward, spacing: Chrome.gap) {
                // Above the block; the corner above the rocker is the tab's.
                HStack(spacing: Chrome.gap) {
                    if side == .left {
                        Color.clear.frame(width: SideMetrics.rockerWidth, height: 0)
                        rows
                    } else {
                        rows
                        Color.clear.frame(width: SideMetrics.rockerWidth, height: 0)
                    }
                }
                HStack(alignment: .bottom, spacing: Chrome.gap) {
                    if side == .left {
                        bar
                        block
                    } else {
                        block
                        bar
                    }
                }
            }
        }
    }

    private var bar: some View {
        rocker()
            .frame(width: SideMetrics.rockerWidth, height: SideMetrics.rocker)
    }

    private var block: some View {
        VStack(spacing: Chrome.gap) {
            selector()
                .frame(height: SideMetrics.selectorHeight)
            under()
                .frame(height: SideMetrics.modeHeight)
        }
        .frame(width: SideMetrics.blockWidth, height: SideMetrics.blockHeight)
    }

    /// The value over the K / Tint row, towards the block.
    private var rows: some View {
        VStack(alignment: inward, spacing: Chrome.gap) {
            // Each row keeps its height when empty.
            Color.clear
                .frame(width: 0, height: SideMetrics.rowHeight)
                .overlay(alignment: side == .left ? .leading : .trailing) { value().fixedSize() }
            Color.clear
                .frame(width: 0, height: SideMetrics.rowHeight)
                .overlay(alignment: side == .left ? .leading : .trailing) { rowSwitch().fixedSize() }
        }
        .frame(width: rowsBeside ? SideMetrics.besideWidth : SideMetrics.blockWidth,
               height: SideMetrics.rowsHeight, alignment: side == .left ? .leading : .trailing)
    }
}

/// Cells in a grid (the selector, the mode switch, the lens buttons): the
/// filled cell is the current one; `onTap` gets the index of the one
/// tapped. Fills the size it is given.
struct CellGrid: View {
    struct Cell {
        let title: String
        let on: Bool
        /// The current cell's fill and text.
        var fill: Color = .white
        var text: Color = .black
    }

    let cells: [Cell]
    let columns: Int
    var fontSize: CGFloat = 15
    let onTap: (Int) -> Void

    var body: some View {
        let rows = stride(from: 0, to: cells.count, by: columns).map { Array($0 ..< min($0 + columns, cells.count)) }
        VStack(spacing: SideMetrics.cellGap) {
            ForEach(rows, id: \.first) { row in
                HStack(spacing: SideMetrics.cellGap) {
                    ForEach(row, id: \.self) { index in
                        cell(cells[index])
                            .onTapGesture { onTap(index) }
                    }
                }
            }
        }
        .padding(SideMetrics.cellGap)
        .sideSurface(RoundedRectangle(cornerRadius: 10))
    }

    private func cell(_ cell: Cell) -> some View {
        Text(cell.title)
            .font(.system(size: fontSize, weight: .bold, design: .rounded))
            .lineLimit(1)
            .minimumScaleFactor(0.7)
            .foregroundStyle(cell.on ? cell.text : .white)
            .shadow(color: .black.opacity(cell.on ? 0 : 0.5), radius: 2)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(cell.on ? cell.fill.opacity(0.7) : .clear, in: RoundedRectangle(cornerRadius: 8))
            .contentShape(RoundedRectangle(cornerRadius: 8))
            .accessibilityElement()
            .accessibilityLabel(cell.title)
            .accessibilityAddTraits(cell.on ? [.isButton, .isSelected] : .isButton)
    }
}

/// The value under a side control's rocker; it grows and turns bright while
/// the rocker is used.
struct SideValueLabel: View {
    let text: String
    var tone: Color = .white
    let adjusting: Bool
    /// Grows from the screen edge inwards.
    var anchor: UnitPoint = .center

    var body: some View {
        Text(text)
            .font(.system(size: 13, weight: .semibold, design: .rounded).monospacedDigit())
            .foregroundStyle(tone.opacity(adjusting ? 1 : 0.9))
            .shadow(color: .black.opacity(0.5), radius: 2)
            .lineLimit(1)
            .fixedSize()
            .padding(.horizontal, 8)
            .frame(height: SideMetrics.rowHeight)
            .sideSurface(Capsule(), dim: adjusting ? 0.45 : 0.15)
            .scaleEffect(adjusting ? 1.25 : 1, anchor: anchor)
            .animation(.easeOut(duration: 0.15), value: adjusting)
            .allowsHitTesting(false)
    }
}
