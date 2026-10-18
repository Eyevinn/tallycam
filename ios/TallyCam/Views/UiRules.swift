import Foundation

// The shooting screen's rules as plain functions, free of SwiftUI, so the
// shared vectors (core/tests/vectors/ui_values.json) check them here and on
// Android (UiRules.kt): the values row, the side controls' value labels,
// the rockers' tap steps and neutral values, and which group each side
// control runs (docs/UI_DESIGN.md).

/// Exposure, colour and focus: the groups of the camera values row and the
/// side controls (the `group` in their descriptors).
enum ShadingGroup: String, CaseIterable, Identifiable {
    case exposure
    case whiteBalance = "white_balance"
    case focus

    var id: String {
        rawValue
    }

    var modeKey: String {
        switch self {
        case .exposure: "exposure_mode"
        case .whiteBalance: "wb_mode"
        case .focus: "focus_mode"
        }
    }

    /// In the side controls' selector.
    var caption: String {
        switch self {
        case .exposure: "EXP"
        case .whiteBalance: "WB"
        case .focus: "FOC"
        }
    }

    var title: String {
        switch self {
        case .exposure: "Exposure"
        case .whiteBalance: "Colour"
        case .focus: "Focus"
        }
    }
}

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
}

/// How a value is drawn: white (auto), yellow (held), or a warning colour.
enum Tone: String {
    case auto
    case held
    case orange
    case red
}

/// One value in the values row.
struct ValueItem: Hashable {
    let text: String
    var flag: String?
    var tone: Tone = .auto
    /// The group a tap on it chooses; nil opens Settings.
    var group: ShadingGroup?
    /// The controls it shows: it flashes when the hub changes one.
    var ids: [String] = []
}

/// One step of the top bar's drop order (`UiRules.dropOrder`).
enum DropStep: String {
    case focus
    case whiteBalance = "white_balance"
    /// The name chip's ON AIR / PREVIEW.
    case tallyLabel = "tally_label"
    case evBias = "ev_bias"
    case shutter

    func drops(_ item: ValueItem) -> Bool {
        switch self {
        case .focus: item.group == .focus
        case .whiteBalance: item.group == .whiteBalance
        case .tallyLabel: false
        case .evBias, .shutter: item.ids.contains(rawValue)
        }
    }
}

/// What a double tap on a rocker's knob does.
enum Neutral: Equatable {
    case set([String: JSONValue])
    case command(String)
}

/// What a shading group's rocker moves now (docs/UI_DESIGN.md, the side
/// control's table): full speed in the control's scale per second, a tap's
/// step in the same scale (log2 for ISO), and the mode to switch to first.
struct Ramped: Equatable {
    let id: String
    let label: String
    let speed: Double
    let tapStep: Double
    var log = false
    /// The group's mode has to change first (a held group goes manual).
    var mode: String?

    func scaled(_ value: Double) -> Double {
        log ? Foundation.log2(value) : value
    }

    func unscaled(_ position: Double) -> Double {
        log ? exp2(position) : position
    }
}

enum UiRules {
    /// Numbers print with a point in every language ("0.42", never "0,42").
    static func fmt(_ pattern: String, _ args: CVarArg...) -> String {
        String(format: pattern, locale: posix, arguments: args)
    }

    private static let posix = Locale(identifier: "en_US_POSIX")

    /// `x` to `places` decimals, halves away from zero (0.25 is "0.3", -0.5
    /// is "-1", as Android), with a sign when `signed`. Rounded before
    /// printing: printf rounds exact halves to even. Never "-0".
    static func number(_ x: Double, _ places: Int, signed: Bool = false) -> String {
        let scale = pow(10, Double(places))
        let value = (x * scale).rounded(.toNearestOrAwayFromZero) / scale + 0.0
        return fmt(signed ? "%+.*f" : "%.*f", Int32(places), value)
    }

    /// "1080p30", "4K30", "720p60": the short side and the frame rate.
    static func formatText(width: Int, height: Int, fps: Int) -> String {
        let lines = min(width, height)
        return (lines >= 2160 ? "4K" : "\(lines)p") + "\(fps)"
    }

    /// What the top bar (and a narrow values row) leaves out when it does
    /// not fit, in this order; it never wraps. Only after all of these is
    /// the camera name shortened.
    static let dropOrder: [DropStep] = [.focus, .whiteBalance, .tallyLabel, .evBias, .shutter]

    /// `items` without what the first `count` steps of `dropOrder` leave out
    /// (the tally label is the name chip's, not an item).
    static func dropping(_ count: Int, from items: [ValueItem]) -> [ValueItem] {
        let dropped = dropOrder.prefix(count)
        return items.filter { item in !dropped.contains { $0.drops(item) } }
    }

    /// The values row (docs/UI_DESIGN.md, "Values"): format, ISO (with CLIP
    /// or DARK from the image statistics' `warning`), shutter, EV (auto and
    /// not 0), white balance (K with the tint when not 0, or AWB without a
    /// temperature) and focus (AF, F 0.42, MF). Only auto and manual: any
    /// other mode (an old hub's `locked`) shows as manual.
    static func valueItems(_ values: [String: JSONValue], format: String, warning: String?) -> [ValueItem] {
        let exposureHeld = values["exposure_mode"]?.string.map { $0 != "auto" } ?? false
        let held: Tone = exposureHeld ? .held : .auto
        var list: [ValueItem] = []
        if !format.isEmpty {
            list.append(ValueItem(text: format, ids: ["resolution", "frame_rate"]))
        }
        if let iso = values["iso"]?.double {
            var item = ValueItem(text: "ISO " + number(iso, 0), tone: held, group: .exposure, ids: ["iso", "exposure_mode"])
            switch warning {
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
            // Int(exactly:): Int(Double) traps outside Int's range.
            let text = Int(exactly: shutter.rounded()).map(String.init) ?? number(shutter, 0)
            list.append(ValueItem(text: "1/" + text, tone: held, group: .exposure, ids: ["shutter"]))
        }
        if !exposureHeld, let bias = values["ev_bias"]?.double, abs(bias) >= 0.05 {
            list.append(ValueItem(text: number(bias, 1, signed: true) + " EV", group: .exposure, ids: ["ev_bias"]))
        }
        if let mode = values["wb_mode"]?.string {
            let ids = ["wb_mode", "wb_temperature", "wb_tint"]
            let tone: Tone = mode == "auto" ? .auto : .held
            if let temperature = values["wb_temperature"]?.double {
                // The tint (magenta +, green −) after the temperature, when not 0.
                let tint = values["wb_tint"]?.double.flatMap { abs($0) >= 0.5 ? " " + number($0, 0, signed: true) : nil } ?? ""
                list.append(ValueItem(text: number(temperature, 0) + " K" + tint, tone: tone, group: .whiteBalance,
                                      ids: ids))
            } else {
                list.append(ValueItem(text: "AWB", tone: tone, group: .whiteBalance, ids: ids))
            }
        }
        if let mode = values["focus_mode"]?.string {
            let ids = ["focus_mode", "focus_position"]
            switch mode {
            case "auto":
                list.append(ValueItem(text: "AF", group: .focus, ids: ids))
            default:
                let text = values["focus_position"]?.double.map { "F " + number($0, 2) } ?? "MF"
                list.append(ValueItem(text: text, tone: .held, group: .focus, ids: ids))
            }
        }
        return list
    }

    /// The value under a shading group's rocker: what it moves, as it
    /// stands ("+0.3 EV", "ISO 400", "5200 K", "Tint +4", "AF", "F 0.42").
    static func adjustValueText(_ group: ShadingGroup, _ values: [String: JSONValue], tint: Bool) -> String {
        let auto = (values[group.modeKey]?.string ?? "auto") == "auto"
        switch group {
        case .exposure:
            if auto {
                return number(values["ev_bias"]?.double ?? 0, 1, signed: true) + " EV"
            }
            return values["iso"]?.double.map { "ISO " + number($0, 0) } ?? "ISO"
        case .whiteBalance:
            if tint {
                return values["wb_tint"]?.double.map { "Tint " + number($0, 0, signed: true) } ?? "Tint"
            }
            return values["wb_temperature"]?.double.map { number($0, 0) + " K" } ?? "AWB"
        case .focus:
            if auto {
                return "AF"
            }
            return values["focus_position"]?.double.map { "F " + number($0, 2) } ?? "F"
        }
    }

    /// The zoom factor against the wide lens, as the lens buttons count it:
    /// "2.3×" below 10, "12×" from 10.
    static func zoomFactorText(_ factor: Double) -> String {
        number(factor, factor >= 10 ? 0 : 1) + "×"
    }

    /// The stream's bitrate in the status row.
    static func bitrate(kbps: Int) -> String {
        kbps >= 1000 ? number(Double(kbps) / 1000, 1) + " Mb/s" : "\(kbps) kb/s"
    }

    /// Time live in the status row: "01:05", "1:02:05".
    static func uptime(seconds: TimeInterval) -> String {
        let total = seconds.isFinite ? max(0, Int(min(seconds, 1e9))) : 0
        let h = total / 3600, m = total / 60 % 60, s = total % 60
        return h > 0 ? fmt("%d:%02d:%02d", h, m, s) : fmt("%02d:%02d", m, s)
    }

    /// The time-of-day timecode badge, "14:51:46:12": local time from
    /// UTC ns (the disciplined clock's), frames at `fps`; "--" frames when
    /// the clock is not synchronised (or no frame rate).
    static func timecode(utcNs: Int64, fps: Int, timeZone: TimeZone = .current, synced: Bool) -> String {
        let seconds = utcNs.quotientAndRemainder(dividingBy: 1_000_000_000)
        // Floor towards earlier times before 1970 too.
        let whole = seconds.remainder < 0 ? seconds.quotient - 1 : seconds.quotient
        let subsecondNs = seconds.remainder < 0 ? seconds.remainder + 1_000_000_000 : seconds.remainder
        let offset = Int64(timeZone.secondsFromGMT(for: Date(timeIntervalSince1970: TimeInterval(whole))))
        let ofDay = ((whole + offset) % 86400 + 86400) % 86400
        let time = fmt("%02d:%02d:%02d", Int(ofDay / 3600), Int(ofDay / 60 % 60), Int(ofDay % 60))
        guard synced, fps > 0 else {
            return time + ":--"
        }
        // Integer arithmetic: floor(subsecond × fps) exactly, as Android.
        let frame = subsecondNs * Int64(fps) / 1_000_000_000
        return time + fmt(":%02d", Int(min(frame, Int64(fps) - 1)))
    }

    // MARK: Rockers

    /// Zoom doublings per second at full deflection, and per tap on an end.
    static let maxZoomRate = 2.0
    static let zoomTap = 0.25

    /// The control a shading group's rocker moves now; nil when the camera
    /// does not have it.
    static func ramped(_ group: ShadingGroup, _ values: [String: JSONValue],
                       _ descriptors: [String: ControlDescriptor], tint: Bool) -> Ramped?
    {
        let mode = values[group.modeKey]?.string ?? "auto"
        let manual = mode == "manual" ? nil : "manual"
        let ramped = switch group {
        case .exposure:
            mode == "auto" ? Ramped(id: "ev_bias", label: "compensation", speed: 2, tapStep: 0.3)
                : Ramped(id: "iso", label: "ISO", speed: 2, tapStep: 1.0 / 3, log: true, mode: manual)
        case .whiteBalance:
            tint ? Ramped(id: "wb_tint", label: "tint", speed: 60, tapStep: 1, mode: manual)
                : Ramped(id: "wb_temperature", label: "temperature", speed: 1500, tapStep: 100, mode: manual)
        case .focus:
            Ramped(id: "focus_position", label: "position", speed: 0.4, tapStep: 0.02, mode: manual)
        }
        return descriptors[ramped.id] == nil ? nil : ramped
    }

    /// `raw` clamped to the control's range and snapped to its step, as the
    /// set to send (with the mode where it has to change); nil without a
    /// range.
    static func rampSet(_ ramped: Ramped, _ group: ShadingGroup, _ descriptors: [String: ControlDescriptor],
                        raw: Double) -> [String: JSONValue]?
    {
        guard let descriptor = descriptors[ramped.id], let low = descriptor.min, let high = descriptor.max,
              high > low, raw.isFinite
        else {
            return nil
        }
        var value = raw
        if let step = descriptor.step, step > 0 {
            value = (value / step).rounded() * step
        }
        value = min(max(value, low), high)
        // Clean decimals: 0.30000000000000004 is 0.3.
        value = (value * 1e6).rounded() / 1e6
        var set: [String: JSONValue] = [ramped.id: .number(value)]
        if let mode = ramped.mode {
            set[group.modeKey] = .string(mode)
        }
        return set
    }

    /// A tap on an end of a rocker: one step up (1) or down (-1) from `base`
    /// (a quick tap's last value) or the camera's value, as the set to send.
    /// Without a value the step starts from the middle of the range. Zoom
    /// moves a quarter doubling, to 0.01.
    static func tapStep(_ group: SideGroup, _ values: [String: JSONValue], _ descriptors: [String: ControlDescriptor],
                        tint: Bool, direction: Int, base: Double? = nil) -> [String: JSONValue]?
    {
        guard let shadingGroup = group.shading else {
            guard let descriptor = descriptors["zoom"], let low = descriptor.min, let high = descriptor.max else {
                return nil
            }
            let current = base ?? values["zoom"]?.double ?? 1
            let zoom = (current * exp2(zoomTap * Double(direction)) * 100).rounded() / 100
            return ["zoom": .number(min(max(zoom, low), high))]
        }
        guard let ramped = ramped(shadingGroup, values, descriptors, tint: tint),
              let descriptor = descriptors[ramped.id], let low = descriptor.min, let high = descriptor.max
        else {
            return nil
        }
        let current = base ?? values[ramped.id]?.double ?? (low + high) / 2
        guard !ramped.log || current > 0 else {
            return nil
        }
        return rampSet(ramped, shadingGroup, descriptors,
                       raw: ramped.unscaled(ramped.scaled(current) + Double(direction) * ramped.tapStep))
    }

    /// A double tap on a rocker's knob: the neutral value (docs/UI_DESIGN.md).
    /// EV 0 in auto exposure, else back to auto; white balance once (K) or
    /// tint 0 in manual white balance (the camera takes a tint nowhere
    /// else); focus back to auto; zoom the lens's own 1×. Nil: nothing to do.
    static func neutral(_ group: SideGroup, _ values: [String: JSONValue], tint: Bool) -> Neutral? {
        guard let shadingGroup = group.shading else {
            return .set(["zoom": .number(1)])
        }
        let mode = values[shadingGroup.modeKey]?.string ?? "auto"
        switch shadingGroup {
        case .whiteBalance where tint:
            return mode == "manual" ? .set(["wb_tint": .number(0)]) : nil
        case .whiteBalance:
            return .command("wb_one_shot")
        case .exposure where mode == "auto":
            return .set(["ev_bias": .number(0)])
        default:
            return mode == "auto" ? nil : .set([shadingGroup.modeKey: .string("auto")])
        }
    }
}

/// What the two side controls run (docs/UI_DESIGN.md, "Side control"):
/// zoom on the left and exposure on the right until changed; never the
/// same group on both sides. A side may be hidden ("–"), folded into a tab
/// at the edge; it keeps its group for when it comes back.
struct Sides: Equatable {
    var left: SideGroup?
    var right: SideGroup?
    var leftHidden = false
    var rightHidden = false

    static let `default` = Sides(left: .zoom, right: .exposure)

    /// The choice where the camera has the group, else the first it has
    /// that the other side does not run.
    func resolved(available: [SideGroup]) -> Sides {
        var sides = self
        sides.right = right.flatMap { available.contains($0) ? $0 : nil } ?? available.first { $0 != left }
        sides.left = left.flatMap { available.contains($0) && $0 != sides.right ? $0 : nil }
            ?? available.first { $0 != sides.right }
        return sides
    }

    /// A selector tap on one side: the chosen group again does nothing; the
    /// other side's group swaps the two (each side keeps whether it is
    /// hidden).
    func choose(_ side: Side, _ group: SideGroup) -> Sides {
        var sides = self
        switch side {
        case .left where group == left, .right where group == right:
            break
        case .left:
            sides.right = group == right ? left : right
            sides.left = group
        case .right:
            sides.left = group == left ? right : left
            sides.right = group
        }
        return sides
    }

    /// A tap on a shown side's tab: it folds away, keeping its group.
    func hide(_ side: Side) -> Sides {
        var sides = self
        if side == .left {
            sides.leftHidden = true
        } else {
            sides.rightHidden = true
        }
        return sides
    }

    /// A tap on a hidden side's tab: it comes back with its group.
    func show(_ side: Side) -> Sides {
        var sides = self
        if side == .left {
            sides.leftHidden = false
        } else {
            sides.rightHidden = false
        }
        return sides
    }

    /// A tap in the values row: nothing while a shown side runs the group; a
    /// hidden side running it comes back; else the group goes to the right,
    /// which shows.
    func fromValues(_ group: SideGroup) -> Sides {
        var sides = self
        if group == left && !leftHidden || group == right && !rightHidden {
            return self
        } else if group == left {
            sides.leftHidden = false
        } else if group == right {
            sides.rightHidden = false
        } else {
            sides.right = group
            sides.rightHidden = false
        }
        return sides
    }

    func group(_ side: Side) -> SideGroup? {
        side == .left ? left : right
    }

    func hidden(_ side: Side) -> Bool {
        side == .left ? leftHidden : rightHidden
    }
}

/// The screen edge a side control sits on; the left one is the right one's
/// mirror image.
enum Side {
    case left
    case right
}
