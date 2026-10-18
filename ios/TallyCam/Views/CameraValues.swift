import SwiftUI

/// The camera's settings in one line under the status line, like a cinema
/// camera's info line: format, ISO, shutter, compensation (auto, when not
/// 0), white balance (with the tint when not 0) and focus. It is also where
/// the operator goes to change them, as on the Blackmagic app
/// (docs/CAMERA_READOUTS.md): ISO, shutter and EV open exposure, K white
/// balance, AF or F focus, the format Settings (`UiRules.valueItems`).
/// A value whose group is manual is yellow, auto is white; the
/// groups on the side controls are underlined in their colour. The exposure warning from the image statistics colours ISO
/// and follows it ("ISO 26 DARK"). Follows the values the hub sets too, a
/// value it just set flashing cyan for a second. On
/// a narrow screen focus goes first, then white balance; the line never
/// wraps.
struct CameraValuesRow: View {
    @ObservedObject var shading: Shading
    @ObservedObject var capture: Capture
    /// The groups on the side controls: their values are underlined in the
    /// group's colour.
    let selected: [ShadingGroup]
    /// A tap on a group's value.
    let onSelect: (ShadingGroup) -> Void
    let onFormat: () -> Void
    /// How many steps of `UiRules.dropOrder` to apply; nil: as many as it
    /// takes to fit.
    var drop: Int?
    /// Inside the landscape top bar: no capsule of its own.
    var embedded = false

    /// A value as drawn: its colour, cyan while the hub's change flashes.
    private struct Item: Hashable {
        let value: ValueItem
        let color: Color
    }

    var body: some View {
        // Redrawn when a flash ends; a new flash redraws through `shading`.
        TimelineView(.explicit(shading.flashEnds())) { context in
            let items = items(now: context.date)
            if let drop {
                line(Self.dropping(drop, from: items))
            } else {
                // Leaving out more and more, in the drop order.
                ViewThatFits(in: .horizontal) {
                    ForEach(0 ... UiRules.dropOrder.count, id: \.self) { count in
                        line(Self.dropping(count, from: items))
                    }
                }
            }
        }
    }

    private func line(_ items: [Item]) -> some View {
        HStack(spacing: 0) {
            ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                if index > 0 {
                    Rectangle()
                        .fill(.white.opacity(0.25))
                        .frame(width: 1, height: 14)
                }
                button(item)
            }
        }
        .font(.system(size: 14, weight: .semibold, design: .rounded).monospacedDigit())
        .lineLimit(1)
        .fixedSize()
        .padding(.horizontal, embedded ? 0 : 4)
        .frame(height: Chrome.rowHeight)
        // Inside the top bar it has the bar's.
        .sideSurface(Capsule(), visible: !embedded)
    }

    private static func dropping(_ count: Int, from items: [Item]) -> [Item] {
        let kept = Set(UiRules.dropping(count, from: items.map(\.value)))
        return items.filter { kept.contains($0.value) }
    }

    private func button(_ item: Item) -> some View {
        let color = item.color, item = item.value
        let selected = item.group.flatMap { self.selected.contains($0) ? $0 : nil }
        let enabled = item.group.map { shading.groups.contains($0) } ?? true
        return Button {
            if let group = item.group {
                onSelect(group)
            } else {
                onFormat()
            }
        } label: {
            HStack(spacing: 4) {
                Text(item.text)
                if let flag = item.flag {
                    Text(flag).fontWeight(.heavy)
                }
            }
            .foregroundStyle(color)
            .padding(.horizontal, 9)
            .frame(height: Chrome.rowHeight)
            .overlay(alignment: .bottom) {
                if let selected {
                    Capsule()
                        .fill(SideGroup(selected).color)
                        .frame(height: 2)
                        .padding(.horizontal, 7)
                        .padding(.bottom, 4)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .accessibilityLabel([item.text, item.flag].compactMap { $0 }.joined(separator: " "))
        .accessibilityHint(item.group.map { "Adjusts \($0.title) on a side control" } ?? "Opens Settings")
    }

    private func items(now: Date) -> [Item] {
        UiRules.valueItems(shading.values, format: formatText, warning: shading.exposureWarning).map { value in
            let tone: Color = switch value.tone {
            case .auto: .white
            case .held: .yellow
            case .orange: .orange
            case .red: .red
            }
            return Item(value: value, color: shading.hubFlash(value.ids, now: now) ? .cyan : tone)
        }
    }

    /// "1080p30", "4K30", "720p60": the streamed picture's short side.
    private var formatText: String {
        let size = capture.streamSize
        return UiRules.formatText(width: Int(size.width), height: Int(size.height), fps: capture.format.frameRate)
    }
}
