import XCTest

/// `DeviceKey.loadOrCreate` against a Keychain that cannot be read now.
final class DeviceKeyStorageTests: XCTestCase {
    private final class FlakyStorage: DeviceKeyStorage {
        var data: Data?
        var unavailable = false
        var deletes = 0
        func load() -> Data? { unavailable ? nil : data }
        func loadResult() -> DeviceKeyLoad {
            if unavailable {
                return .unavailable(errSecInteractionNotAllowed)
            }
            return data.map(DeviceKeyLoad.found) ?? .notFound
        }
        func save(_ data: Data) throws { self.data = data }
        func delete() {
            deletes += 1
            data = nil
        }
    }

    private var defaults: UserDefaults!
    private let suite = "DeviceKeyStorageTests"

    override func setUp() {
        UserDefaults().removePersistentDomain(forName: suite)
        defaults = UserDefaults(suiteName: suite)
    }

    override func tearDown() {
        UserDefaults().removePersistentDomain(forName: suite)
    }

    func testAnUnreadableKeychainKeepsTheStoredKey() {
        let storage = FlakyStorage()
        let first = DeviceKey.loadOrCreate(storage: storage, defaults: defaults, secureEnclave: false)!
        let stored = storage.data
        let deletesAfterCreate = storage.deletes

        // Before the first unlock after a reboot: no key at all (the caller
        // waits), never a temporary one that a server sees as another camera.
        storage.unavailable = true
        XCTAssertNil(DeviceKey.loadOrCreate(storage: storage, defaults: defaults, secureEnclave: false))
        XCTAssertEqual(storage.data, stored, "the stored key is not replaced")
        XCTAssertEqual(storage.deletes, deletesAfterCreate)
        XCTAssertTrue(defaults.bool(forKey: DeviceKey.installMarker))

        // Unlocked again: the same camera.
        storage.unavailable = false
        let unlocked = DeviceKey.loadOrCreate(storage: storage, defaults: defaults, secureEnclave: false)
        XCTAssertEqual(unlocked?.deviceId, first.deviceId)
    }

    func testAMissingKeyIsReplaced() {
        let storage = FlakyStorage()
        let first = DeviceKey.loadOrCreate(storage: storage, defaults: defaults, secureEnclave: false)!
        storage.data = nil
        let second = DeviceKey.loadOrCreate(storage: storage, defaults: defaults, secureEnclave: false)!
        XCTAssertNotEqual(second.deviceId, first.deviceId)
        XCTAssertNotNil(storage.data)
    }
}
