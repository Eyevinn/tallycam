import Foundation

/// Which mains frequency applies (docs/SHADING.md, "Shutter"): the phone's
/// own (saved), the hub production's, and a change made since the
/// production last changed. The last write wins; the hub never locks it.
///
/// - A change on the phone (Settings, or the control set locally) saves the
///   phone's own value and holds over the production, even when it equals
///   what is in effect.
/// - A change from the hub (a per-camera `mains_hz` set) holds for this
///   session only and is never saved as the phone's own.
/// - A new hub production clears the held change; leaving the hub returns
///   to the phone's own.
struct Mains: Equatable {
    /// The phone's own value, saved in its Settings.
    var own: Int
    /// The hub production's, while one applies.
    private(set) var production: Int?
    /// Set since the production last changed; never saved.
    private(set) var held: Int?

    init(own: Int) {
        self.own = own
    }

    /// What applies now.
    var effective: Int {
        held ?? production ?? own
    }

    /// A set of `mains_hz`. Returns true when the phone's own value is to
    /// be saved.
    @discardableResult
    mutating func set(_ hz: Int, fromHub: Bool) -> Bool {
        held = hz
        guard !fromHub else {
            return false
        }
        own = hz
        return true
    }

    /// The hub's production's mains (nil: none). A different one clears
    /// the held change; the same one again (every reconnect) keeps it.
    mutating func production(_ hz: Int?, changed: Bool) {
        if changed {
            held = nil
        }
        production = hz
    }

    /// No hub any more (forgotten, or another one joined).
    mutating func leaveHub() {
        held = nil
        production = nil
    }
}
