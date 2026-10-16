import CryptoKit
import Foundation
import os
import Security

private let log = Logger(subsystem: "se.eyevinn.tallycam", category: "devicekey")

/// The phone's device key: a P-256 key pair that proves to a server which
/// camera this is (docs/PROTOCOL.md, "Device keys").
///
/// Only what must be native lives here: creating and keeping the key (in
/// the Secure Enclave on a device, in software on the simulator) and signing.
/// The id, the hello field, the bytes to sign and the `auth` message come
/// from the shared core (`deviceKey*` in the UniFFI bindings), the same code
/// the server verifies with.
///
/// The key is created on first launch and kept in the Keychain, this device
/// only (never in a backup). A reinstall creates a new key, so a reinstalled
/// app is a new camera that needs a new invite.
final class DeviceKey: @unchecked Sendable {
    enum Failure: Error {
        case keychain(OSStatus)
        case core(String)
    }

    private static let lock = NSLock()
    private static var resolved: DeviceKey?

    /// The app's key, created on first use; nil while the Keychain cannot be
    /// read (before the first unlock after a reboot) or written. Nothing that
    /// needs the device id may run before this returns a key: the caller
    /// waits and asks again (see `RemoteControl`), so a temporary key never
    /// shows up on a server as another camera.
    static func current() -> DeviceKey? {
        lock.withLock {
            if let resolved {
                return resolved
            }
            resolved = loadOrCreate(storage: KeychainKeyStorage(), defaults: .standard,
                                    secureEnclave: SecureEnclave.isAvailable)
            return resolved
        }
    }

    /// UserDefaults key that marks this install as having created its key.
    /// UserDefaults go with an uninstall, Keychain items stay: without the
    /// marker a key found in the Keychain is from an earlier install and is
    /// replaced.
    static let installMarker = "deviceKeyCreated"

    let signer: Signer
    /// From the core: `hello.public_key`, `hello.device.id`, fingerprint.
    let identity: DeviceKeyIdentity

    var deviceId: String {
        identity.deviceId
    }

    private init(signer: Signer, identity: DeviceKeyIdentity) {
        self.signer = signer
        self.identity = identity
    }

    convenience init(signer: Signer) throws {
        do {
            try self.init(signer: signer, identity: deviceKeyIdentity(publicKeyX963: signer.publicKeyX963))
        } catch {
            throw Failure.core("\(error)")
        }
    }

    /// "3f2a 9c1d 7b0e 44a1": the start of a fingerprint, as the operator
    /// page shows it.
    static func shortFingerprint(_ fingerprint: String) -> String {
        var groups: [Substring] = []
        var rest = fingerprint.prefix(16)[...]
        while !rest.isEmpty {
            groups.append(rest.prefix(4))
            rest = rest.dropFirst(4)
        }
        return groups.joined(separator: " ")
    }

    /// The `auth` message (JSON text) answering a `challenge`.
    func authMessage(server: String, nonce: String) throws -> String {
        let payload = try deviceKeyAuthPayload(server: server, nonce: nonce, deviceId: identity.deviceId)
        let signature = try signer.sign(payload)
        return try deviceKeyAuthMessage(signature: signature)
    }

    /// The stored key, or a new one; nil when the Keychain cannot be used
    /// right now. Only a key that is not there, or not usable, is replaced.
    /// A Keychain that cannot be read (before the first unlock after a
    /// reboot, `errSecInteractionNotAllowed`) keeps the stored key untouched
    /// and returns nil, as does one that cannot store a new key: the caller
    /// tries again later rather than run on a key of the moment, which a
    /// server would see as another camera.
    static func loadOrCreate(storage: DeviceKeyStorage, defaults: UserDefaults, secureEnclave: Bool) -> DeviceKey? {
        if defaults.bool(forKey: installMarker) {
            switch storage.loadResult() {
            case let .found(data):
                do {
                    return try DeviceKey(signer: Signer(stored: data))
                } catch {
                    log.error("Stored device key unusable (\(String(describing: error), privacy: .public)), creating a new one")
                }
            case .notFound:
                log.error("Device key missing from the Keychain, creating a new one")
            case let .unavailable(status):
                log.info("Keychain not readable yet (\(status)); waiting for the stored device key")
                return nil
            }
        }
        storage.delete()
        do {
            let signer = try Signer.create(secureEnclave: secureEnclave)
            try storage.save(signer.storedRepresentation)
            defaults.set(true, forKey: installMarker)
            let key = try DeviceKey(signer: signer)
            log.info("Created device key \(key.identity.deviceId, privacy: .public)\(secureEnclave ? " in the Secure Enclave" : "")")
            return key
        } catch {
            log.error("Cannot keep a device key (\(String(describing: error), privacy: .public)); will try again")
            return nil
        }
    }

    /// A P-256 signing key.
    enum Signer {
        case secureEnclave(SecureEnclave.P256.Signing.PrivateKey)
        case software(P256.Signing.PrivateKey)

        /// Stored form: one tag byte, then CryptoKit's representation (the
        /// Secure Enclave's wrapped key blob, usable only on this device, or
        /// the software key's raw scalar).
        private static let secureEnclaveTag: UInt8 = 1
        private static let softwareTag: UInt8 = 2

        static func create(secureEnclave: Bool) throws -> Signer {
            guard secureEnclave else {
                return .software(P256.Signing.PrivateKey())
            }
            var error: Unmanaged<CFError>?
            guard let access = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
                                                               .privateKeyUsage, &error)
            else {
                throw error!.takeRetainedValue() as Error
            }
            return try .secureEnclave(SecureEnclave.P256.Signing.PrivateKey(accessControl: access))
        }

        init(stored data: Data) throws {
            guard let tag = data.first else {
                throw Failure.keychain(errSecDecode)
            }
            let body = data.dropFirst()
            switch tag {
            case Self.secureEnclaveTag:
                self = try .secureEnclave(SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: body))
            case Self.softwareTag:
                self = try .software(P256.Signing.PrivateKey(rawRepresentation: body))
            default:
                throw Failure.keychain(errSecDecode)
            }
        }

        var storedRepresentation: Data {
            switch self {
            case let .secureEnclave(key): Data([Self.secureEnclaveTag]) + key.dataRepresentation
            case let .software(key): Data([Self.softwareTag]) + key.rawRepresentation
            }
        }

        /// SEC1 uncompressed public key (65 bytes).
        var publicKeyX963: Data {
            switch self {
            case let .secureEnclave(key): key.publicKey.x963Representation
            case let .software(key): key.publicKey.x963Representation
            }
        }

        /// ECDSA P-256 with SHA-256 over `data`: raw `r || s`.
        func sign(_ data: Data) throws -> Data {
            switch self {
            case let .secureEnclave(key): try key.signature(for: data).rawRepresentation
            case let .software(key): try key.signature(for: data).rawRepresentation
            }
        }
    }
}

/// What reading the stored key gave.
enum DeviceKeyLoad: Equatable {
    case found(Data)
    /// No stored key: a new one may replace it.
    case notFound
    /// The store could not be read now (for example a locked device); the
    /// key may well be there, so it must not be replaced.
    case unavailable(OSStatus)
}

/// Where the key's stored form is kept.
protocol DeviceKeyStorage {
    func load() -> Data?
    /// Tells a missing key from a store that cannot be read now. The default
    /// (from `load`) knows no failures.
    func loadResult() -> DeviceKeyLoad
    func save(_ data: Data) throws
    func delete()
}

extension DeviceKeyStorage {
    func loadResult() -> DeviceKeyLoad {
        load().map(DeviceKeyLoad.found) ?? .notFound
    }
}

/// A generic password item, readable after the first unlock (a camera
/// reconnects in the background), never synced or backed up.
struct KeychainKeyStorage: DeviceKeyStorage {
    var service = "se.eyevinn.tallycam.devicekey"
    var account = "device-key-v1"

    private var query: [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: service,
         kSecAttrAccount as String: account]
    }

    func load() -> Data? {
        if case let .found(data) = loadResult() {
            return data
        }
        return nil
    }

    func loadResult() -> DeviceKeyLoad {
        var q = query
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        let status = SecItemCopyMatching(q as CFDictionary, &out)
        switch status {
        case errSecSuccess:
            guard let data = out as? Data else {
                return .unavailable(errSecDecode)
            }
            return .found(data)
        case errSecItemNotFound:
            return .notFound
        default:
            return .unavailable(status)
        }
    }

    func save(_ data: Data) throws {
        var q = query
        q[kSecValueData as String] = data
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(q as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw DeviceKey.Failure.keychain(status)
        }
    }

    func delete() {
        SecItemDelete(query as CFDictionary)
    }
}
