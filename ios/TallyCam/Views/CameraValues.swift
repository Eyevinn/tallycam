import SwiftUI

/// The camera's settings in one line under the status line, like a cinema
/// camera's info line: format, ISO, shutter, compensation (auto, when not
/// 0), white balance (with the tint when not 0) and focus. It is also where
/// the operator goes to change them, as on the Blackmagic app
/// (docs/CAMERA_READOUTS.md): ISO, shutter and EV open exposure, K white
/// balance, AF, AF-L or F focus, the format Settings. A value whose group is
/// locked or manual is yellow, auto is white; the group being adjusted is
/// underlined. The exposure warning from the image statistics colours ISO
/// and follows it ("ISO 26 DARK"). Follows the values the hub sets too, a
/// value it just set flashing cyan for a second. On
/// a narrow screen focus goes first, then white balance; the line never
/// wraps.
struct CameraValuesRow: View {
    @ObservedObject var shading: Shading
    @ObservedObject var capture: Capture
    /// The group the adjust control at the right edge changes.
    @Binding var selection: ShadingGroup
    let onFormat: () -> Void

    private struct Item: Hashable {
        let text: String
        var flag: String?
        var tone: Color = .white
        /// The group it opens; nil opens Settings.
        var group: ShadingGroup?
        /// The controls it shows, for the hub's flash.
        var ids: [String] = []
    }

    var body: some View {
        // Redrawn when a flash ends; a new flash redraws through `shading`.
        TimelineView(.explicit(shading.flashEnds())) { context in
            let items = items(now: context.date)
            ViewThatFits(in: .horizontal) {
                line(items.all)
                line(items.withoutFocus)
                line(items.withoutFocus.filter { $0 != items.colour })
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
        .padding(.horizontal, 4)
        .frame(height: Chrome.rowHeight)
        .background(.black.opacity(0.45), in: Capsule())
    }

    private func button(_ item: Item) -> some View {
        let selected = item.group != nil && item.group == selection
        let enabled = item.group.map { shading.groups.contains($0) } ?? true
        return Button {
            if let group = item.group {
                selection = group
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
            .foregroundStyle(item.tone)
            .padding(.horizontal, 9)
            .frame(height: Chrome.rowHeight)
            .overlay(alignment: .bottom) {
                if selected {
                    Capsule()
                        .fill(.white)
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
        .accessibilityHint(item.group.map { "Adjusts \($0.title) at the right edge" } ?? "Opens Settings")
    }

    private func items(now: Date) -> (all: [Item], withoutFocus: [Item], colour: Item?) {
        var (all, withoutFocus, colour) = items
        func flash(_ item: Item) -> Item {
            var item = item
            if shading.hubFlash(item.ids, now: now) {
                item.tone = .cyan
            }
            return item
        }
        all = all.map(flash)
        withoutFocus = withoutFocus.map(flash)
        colour = colour.map(flash)
        return (all, withoutFocus, colour)
    }

    private var items: (all: [Item], withoutFocus: [Item], colour: Item?) {
        let values = shading.values
        let exposureHeld = shading.mode(.exposure).map { $0 != "auto" } ?? false
        let held: Color = exposureHeld ? .yellow : .white
        var list: [Item] = [Item(text: formatText)]
        if let iso = values["iso"]?.double {
            var item = Item(text: String(format: "ISO %.0f", iso), tone: held, group: .exposure,
                            ids: ["iso", "exposure_mode"])
            switch shading.exposureWarning {
            case "too_dark":
                item.flag = "DARK"
                item.tone = .orange
            case "clipping":
                item.flag = "CLIP"
                item.tone = .red
            default:
                break
            }
            list.append(item)
        }
        if let shutter = values["shutter"]?.double, shutter > 0 {
            list.append(Item(text: "1/\(Int(shutter.rounded()))", tone: held, group: .exposure,
                             ids: ["shutter"]))
        }
        if !exposureHeld, let bias = values["ev_bias"]?.double, abs(bias) >= 0.05 {
            list.append(Item(text: String(format: "%+.1f EV", bias), group: .exposure, ids: ["ev_bias"]))
        }
        // The tint (magenta +, green −) after the temperature, when not 0.
        let tint = values["wb_tint"]?.double.flatMap { abs($0) >= 0.5 ? String(format: " %+.0f", $0) : nil } ?? ""
        let colour = values["wb_temperature"]?.double.map {
            Item(text: String(format: "%.0f K", $0) + tint,
                 tone: shading.mode(.whiteBalance).map { $0 != "auto" } ?? false ? .yellow : .white,
                 group: .whiteBalance, ids: ["wb_temperature", "wb_tint", "wb_mode"])
        }
        if let colour {
            list.append(colour)
        }
        let withoutFocus = list
        switch shading.mode(.focus) {
        case "auto":
            list.append(Item(text: "AF", group: .focus, ids: Self.focusIds))
        case "locked":
            list.append(Item(text: "AF-L", tone: .yellow, group: .focus, ids: Self.focusIds))
        case .some:
            let position = values["focus_position"]?.double
            list.append(Item(text: position.map { String(format: "F %.2f", $0) } ?? "MF", tone: .yellow,
                             group: .focus, ids: Self.focusIds))
        case nil:
            break
        }
        return (list, withoutFocus, colour)
    }

    private static let focusIds = ["focus_mode", "focus_position"]

    /// "1080p30", "4K30", "720p60": the streamed picture's short side.
    private var formatText: String {
        let size = capture.streamSize
        let lines = min(size.width, size.height)
        return (lines >= 2160 ? "4K" : "\(lines)p") + "\(capture.format.frameRate)"
    }
}
