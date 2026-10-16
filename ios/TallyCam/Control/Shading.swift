import Combine
import Foundation
#if canImport(TallyCamCoreFFI)
    import TallyCamCoreFFI
#endif

/// Who owns the camera's exposure, colour and focus (docs/SHADING.md): the
/// core's ownership state machine, persisted across restarts, plus what the
/// operator's screen shows about it and the latest image statistics.
/// `RemoteControl` drives it; the views read it.
@MainActor
final class Shading: ObservableObject {
    static let groups = ["exposure", "white_balance", "focus"]
    private static let stateKey = "shadingOwnership"
    private static let valuesKey = "shadingValues"

    /// One entry per group the camera has, in a fixed order.
    @Published private(set) var groups: [GroupView] = []
    /// `too_dark` or `clipping`, from the image statistics.
    @Published private(set) var exposureWarning: String?
    @Published private(set) var stats: ImageStatsResult?
    /// The camera's control values and descriptors, for the operator's
    /// group controls (refreshed with every `state`).
    @Published private(set) var values: [String: JSONValue] = [:]
    @Published private(set) var descriptors: [ControlDescriptor] = []

    let machine: OwnershipMachine
    /// Called after an owner changes, with the `ownership` message.
    var onChange: ((String) -> Void)?
    /// The group controls' values while a group is not auto, saved so a
    /// relaunch in the middle of a show comes back with them (D3).
    private var savedValues: [String: JSONValue]

    init() {
        machine = OwnershipMachine(groups: Self.groups,
                                   savedJson: UserDefaults.standard.string(forKey: Self.stateKey))
        savedValues = (UserDefaults.standard.data(forKey: Self.valuesKey))
            .flatMap { try? JSONDecoder().decode([String: JSONValue].self, from: $0) } ?? [:]
        groups = machine.view()
    }

    func view(_ group: String) -> GroupView? {
        groups.first { $0.group == group }
    }

    /// The owner of a group changed (or the link did): persist, refresh the
    /// view and tell the server.
    func changed(notify: Bool = true) {
        UserDefaults.standard.set(machine.saveJson(), forKey: Self.stateKey)
        let view = machine.view()
        if view != groups {
            groups = view
        }
        if notify {
            onChange?(machine.messageJson())
        }
    }

    // MARK: Values

    /// The values that put a group into auto (rule 4: never the last values).
    static func autoValues(_ groups: [String]) -> [String: JSONValue] {
        var values: [String: JSONValue] = [:]
        for group in groups {
            switch group {
            case "exposure":
                values["exposure_mode"] = .string("auto")
                values["ev_bias"] = .number(0)
            case "white_balance":
                values["wb_mode"] = .string("auto")
            case "focus":
                values["focus_mode"] = .string("auto")
            default:
                break
            }
        }
        return values
    }

    func update(descriptors: [ControlDescriptor]) {
        if descriptors != self.descriptors {
            self.descriptors = descriptors
        }
    }

    func descriptor(_ id: String) -> ControlDescriptor? {
        descriptors.first { $0.id == id }
    }

    /// Keeps the values of the groups that are not auto.
    func note(values: [String: JSONValue], shutterFree: Bool) {
        if values != self.values {
            self.values = values
        }
        let owned = Set(groups.filter { $0.indicator != "auto" }.map(\.group))
        let kept = values.filter { key, _ in
            shadingControlGroup(control: key, shutterFree: shutterFree).map(owned.contains) ?? false
        }
        guard kept != savedValues else {
            return
        }
        savedValues = kept
        UserDefaults.standard.set(try? JSONEncoder().encode(kept), forKey: Self.valuesKey)
    }

    /// The saved values of the groups that are not auto, in an order the
    /// camera can apply (`CameraControls.apply` sorts by its table).
    func restoredValues() -> [String: JSONValue] {
        let owned = Set(groups.filter { $0.indicator != "auto" }.map(\.group))
        return savedValues.filter { key, _ in
            shadingControlGroup(control: key, shutterFree: true).map(owned.contains) ?? false
        }
    }

    // MARK: Statistics

    func update(stats: ImageStatsResult) {
        self.stats = stats
        if stats.warning != exposureWarning {
            exposureWarning = stats.warning
        }
    }
}

extension GroupView {
    /// Short name on the phone's chips.
    var shortName: String {
        switch group {
        case "exposure": "EXP"
        case "white_balance": "WB"
        case "focus": "FOC"
        default: group.uppercased()
        }
    }

    var title: String {
        switch group {
        case "exposure": "Exposure"
        case "white_balance": "Colour"
        case "focus": "Focus"
        default: group
        }
    }

    var isRemote: Bool {
        indicator == "remote" || indicator == "remote_disconnected"
    }
}

extension String {
    /// "exposure: under remote control" -> "Exposure: under remote control".
    var capitalizedFirst: String {
        prefix(1).uppercased() + dropFirst()
    }
}
