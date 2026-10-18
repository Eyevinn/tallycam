import SwiftUI

/// What a side control runs (docs/UI_DESIGN.md, "Side control").
enum SideGroup: String, CaseIterable, Identifiable {
    case zoom
    case exposure
    case whiteBalance = "white_balance"
    case focus

    var id: String {
        rawValue
    }

    init(_ group: ShadingGroup) {
        switch group {
        case .exposure: self = .exposure
        case .whiteBalance: self = .whiteBalance
        case .focus: self = .focus
        }
    }

    var shading: ShadingGroup? {
        switch self {
        case .zoom: nil
        case .exposure: .exposure
        case .whiteBalance: .whiteBalance
        case .focus: .focus
        }
    }

    var caption: String {
        shading?.caption ?? "ZOOM"
    }

    var title: String {
        shading?.title ?? "Zoom"
    }

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

/// The screen edge a side control sits on; the left one is the right one's
/// mirror image.
enum Side {
    case left
    case right
}

/// The sizes both side controls share, whatever group they run, so that
/// nothing moves when a group changes (docs/UI_DESIGN.md, "Side control",
/// "Sizes"): cells easy to hit without looking, and a rocker as tall as the
/// side's zone allows. Worked out from the zone's height only.
struct SideMetrics {
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
    /// The value label's and the K / Tint switch's rows.
    static let rowHeight = Chrome.rowHeight - 6
    static let rowsHeight = 2 * rowHeight + Chrome.gap
    /// The value and K / Tint column beside the block (a landscape phone).
    static let besideWidth: CGFloat = 104

    /// The rocker's length.
    let rocker: CGFloat
    /// The value and K / Tint rows beside the block (centred on the rocker)
    /// instead of under the rocker, where the stack does not fit.
    let beside: Bool

    /// Under the rocker where the stack fits, the rocker taking the rest;
    /// else beside the block, the rocker the full height.
    static func fitting(height: CGFloat) -> SideMetrics {
        let stacked = height - Chrome.gap - rowsHeight
        if stacked >= blockHeight {
            return SideMetrics(rocker: stacked, beside: false)
        }
        return SideMetrics(rocker: max(height, blockHeight), beside: true)
    }

    var width: CGFloat {
        Self.blockWidth + Chrome.gap + Self.rockerWidth + (beside ? Chrome.gap + Self.besideWidth : 0)
    }
}

/// One side control (docs/UI_DESIGN.md, "Side control"): the rocker at the
/// screen edge and, on its inner side, the selector over the mode switch or
/// lens buttons; the value and, for white balance, the K / Tint switch under
/// the rocker (beside the block on a landscape phone). Runs zoom
/// (`ZoomControl`) or a shading group (`AdjustControl`). Takes its zone's
/// full height, centred in it.
struct SideControl: View {
    let side: Side
    /// This side's group.
    @Binding var group: SideGroup
    /// The other side's: choosing it swaps the two.
    @Binding var other: SideGroup
    let remote: RemoteControl
    let controls: CameraControls
    @ObservedObject var shading: Shading

    /// The zone's height, measured; the same on both sides.
    @State private var zoneHeight: CGFloat = 0

    var body: some View {
        let metrics = SideMetrics.fitting(height: zoneHeight)
        Group {
            if let shadingGroup = group.shading {
                AdjustControl(remote: remote, shading: shading, group: shadingGroup, side: side, metrics: metrics) {
                    selector
                }
            } else {
                ZoomControl(controls: controls, shading: shading, side: side, metrics: metrics) {
                    selector
                }
            }
        }
        // A new group starts with fresh state (the K / Tint switch on K).
        .id(group)
        .frame(width: metrics.width)
        .frame(maxHeight: .infinity)
        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { zoneHeight = $0 }
    }

    private var selector: some View {
        CellGrid(cells: SideGroup.allCases.map {
            .init(title: $0.caption, on: $0 == group, fill: $0.color, text: .white)
        }, columns: 2, fontSize: 13)
        { index in
            let chosen = SideGroup.allCases[index]
            guard chosen != group else {
                return
            }
            if chosen == other {
                other = group
            }
            group = chosen
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(side == .left ? "Left" : "Right") control: \(group.title)")
    }
}

/// The skeleton both kinds of side control share, mirrored on the left.
/// Every slot has a fixed size; what does not fit its slot (a wide value)
/// overflows it without moving anything.
struct SideLayout<Selector: View, Under: View, Bar: View, Value: View, Switch: View>: View {
    let side: Side
    let metrics: SideMetrics
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
        if metrics.beside {
            HStack(spacing: Chrome.gap) {
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
                HStack(alignment: .top, spacing: Chrome.gap) {
                    if side == .left {
                        bar
                        block
                    } else {
                        block
                        bar
                    }
                }
                rows
            }
        }
    }

    private var bar: some View {
        rocker()
            .frame(width: SideMetrics.rockerWidth, height: metrics.rocker)
    }

    /// The selector over the mode row, at the top of the rocker's height.
    private var block: some View {
        VStack(spacing: Chrome.gap) {
            selector()
                .frame(height: SideMetrics.selectorHeight)
            under()
                .frame(height: SideMetrics.modeHeight)
        }
        .frame(width: SideMetrics.blockWidth, height: metrics.rocker, alignment: .top)
    }

    /// The value over the K / Tint row: under the rocker towards the edge,
    /// or beside the block towards it.
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
        .fixedSize()
        .frame(width: metrics.beside ? SideMetrics.besideWidth : metrics.width,
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
        .background(.black.opacity(0.45), in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.white.opacity(0.25), lineWidth: 1))
    }

    private func cell(_ cell: Cell) -> some View {
        Text(cell.title)
            .font(.system(size: fontSize, weight: .bold, design: .rounded))
            .lineLimit(1)
            .minimumScaleFactor(0.7)
            .foregroundStyle(cell.on ? cell.text : .white.opacity(0.8))
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(cell.on ? cell.fill : .clear, in: RoundedRectangle(cornerRadius: 8))
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
            .foregroundStyle(tone.opacity(adjusting ? 1 : 0.8))
            .lineLimit(1)
            .fixedSize()
            .padding(.horizontal, 8)
            .frame(height: SideMetrics.rowHeight)
            .background(.black.opacity(adjusting ? 0.75 : 0.55), in: Capsule())
            .scaleEffect(adjusting ? 1.25 : 1, anchor: anchor)
            .animation(.easeOut(duration: 0.15), value: adjusting)
            .allowsHitTesting(false)
    }
}
