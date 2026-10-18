import Combine
import Foundation
#if canImport(TallyCamCoreFFI)
    import TallyCamCoreFFI
#endif

/// The camera's exposure, colour and focus as the operator's screen shows
/// them (docs/SHADING.md): the control values and descriptors, and the latest
/// image statistics. The operator and the hub may both change them at any
/// time; the last change wins. `RemoteControl` feeds it; the views read it.
@MainActor
final class Shading: ObservableObject {
    /// `too_dark` or `clipping`, from the image statistics.
    @Published private(set) var exposureWarning: String?
    @Published private(set) var stats: ImageStatsResult?
    /// The camera's control values and descriptors, for the operator's
    /// group controls (refreshed with every `state`).
    @Published private(set) var values: [String: JSONValue] = [:]
    @Published private(set) var descriptors: [ControlDescriptor] = []
    /// When the hub last set each control (its `set` messages only): the
    /// values row flashes them cyan for `flashTime`.
    @Published private(set) var hubSetAt: [String: Date] = [:]
    static let flashTime: TimeInterval = 1

    /// The groups the camera has (its mode control exists), in a fixed order.
    var groups: [ShadingGroup] {
        ShadingGroup.allCases.filter { descriptor($0.modeKey) != nil }
    }

    /// The group's mode (`auto`, `locked`, `manual`).
    func mode(_ group: ShadingGroup) -> String? {
        values[group.modeKey]?.string
    }

    func update(descriptors: [ControlDescriptor]) {
        if descriptors != self.descriptors {
            self.descriptors = descriptors
        }
    }

    func descriptor(_ id: String) -> ControlDescriptor? {
        descriptors.first { $0.id == id }
    }

    /// The hub set these controls just now.
    func hubSet(_ ids: some Sequence<String>) {
        let now = Date.now
        for id in ids {
            hubSetAt[id] = now
        }
    }

    /// The hub set one of these controls within `flashTime`.
    func hubFlash(_ ids: [String], now: Date = .now) -> Bool {
        ids.contains { hubSetAt[$0].map { now.timeIntervalSince($0) < Self.flashTime } ?? false }
    }

    func update(values: [String: JSONValue]) {
        if values != self.values {
            self.values = values
        }
    }

    // MARK: Statistics

    func update(stats: ImageStatsResult) {
        self.stats = stats
        if stats.warning != exposureWarning {
            exposureWarning = stats.warning
        }
    }

    #if DEBUG
        /// A made-up warning for the layout preview (`-uiPreviewState`).
        func preview(warning: String?) {
            exposureWarning = warning
        }
    #endif
}

/// Exposure, colour and focus: the groups of the camera values row and the
/// adjust strip (the `group` in their descriptors).
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

    var title: String {
        switch self {
        case .exposure: "Exposure"
        case .whiteBalance: "Colour"
        case .focus: "Focus"
        }
    }
}
