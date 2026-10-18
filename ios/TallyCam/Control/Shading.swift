import Combine
import Foundation
#if canImport(TallyCamCoreFFI)
    import TallyCamCoreFFI
#endif

/// The camera's exposure, colour and focus as the operator's screen shows
/// them (docs/SHADING.md): the control values and descriptors, and the
/// exposure warning from the image statistics. The operator and the hub may both change them at any
/// time; the last change wins. `RemoteControl` feeds it; the views read it.
@MainActor
final class Shading: ObservableObject {
    /// `too_dark` or `clipping`, from the image statistics.
    @Published private(set) var exposureWarning: String?
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
            descriptorMap = Dictionary(descriptors.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
            self.descriptors = descriptors
        }
    }

    /// The descriptors by id, for `UiRules`; built once per change.
    private(set) var descriptorMap: [String: ControlDescriptor] = [:]

    func descriptor(_ id: String) -> ControlDescriptor? {
        descriptorMap[id]
    }

    /// The hub set these controls just now.
    func hubSet(_ ids: some Sequence<String>) {
        let now = Date.now
        for id in ids {
            hubSetAt[id] = now
        }
    }

    /// When the running flashes end, in order: the values row redraws then
    /// (and not at all while nothing flashes).
    func flashEnds(now: Date = .now) -> [Date] {
        hubSetAt.values.map { $0.addingTimeInterval(Self.flashTime) }.filter { $0 > now }.sorted()
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
