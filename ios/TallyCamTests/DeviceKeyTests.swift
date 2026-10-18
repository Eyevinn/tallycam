import CryptoKit
import XCTest

final class DeviceKeyTests: XCTestCase {
    /// Keeps the stored key in memory (the hostless test bundle has no
    /// Keychain access).
    private final class MemoryStorage: DeviceKeyStorage {
        var data: Data?
        func load() -> Data? { data }
        func save(_ data: Data) throws { self.data = data }
        func delete() { data = nil }
    }

    private var defaults: UserDefaults!
    private let suite = "DeviceKeyTests"

    override func setUp() {
        UserDefaults().removePersistentDomain(forName: suite)
        defaults = UserDefaults(suiteName: suite)
    }

    override func tearDown() {
        UserDefaults().removePersistentDomain(forName: suite)
    }

    func testKeyIsKeptAndAReinstallMakesANewOne() {
        let storage = MemoryStorage()
        let first = DeviceKey.loadOrCreate(storage: storage, defaults: defaults, secureEnclave: false)!
        XCTAssertNotNil(storage.data)
        XCTAssertTrue(defaults.bool(forKey: DeviceKey.installMarker))
        let again = DeviceKey.loadOrCreate(storage: storage, defaults: defaults, secureEnclave: false)!
        XCTAssertEqual(again.deviceId, first.deviceId)

        // Reinstall: the Keychain item survived, the defaults did not.
        defaults.removeObject(forKey: DeviceKey.installMarker)
        let reinstalled = DeviceKey.loadOrCreate(storage: storage, defaults: defaults, secureEnclave: false)!
        XCTAssertNotEqual(reinstalled.deviceId, first.deviceId)
    }

    func testIdentityComesFromThePublicKey() throws {
        let key = DeviceKey.loadOrCreate(storage: MemoryStorage(), defaults: defaults, secureEnclave: false)!
        let identity = key.identity
        let x963 = try XCTUnwrap(Data(base64Encoded: identity.publicKey))
        XCTAssertEqual(x963, key.signer.publicKeyX963)
        XCTAssertEqual(x963.count, 65)
        let fingerprint = SHA256.hash(data: x963).map { String(format: "%02x", $0) }.joined()
        XCTAssertEqual(identity.fingerprint, fingerprint)
        XCTAssertEqual(identity.deviceId, String(fingerprint.prefix(32)))
        XCTAssertEqual(DeviceKey.shortFingerprint("0123456789abcdef0011"), "0123 4567 89ab cdef")
    }

    func testAuthMessageVerifies() throws {
        let key = DeviceKey.loadOrCreate(storage: MemoryStorage(), defaults: defaults, secureEnclave: false)!
        let text = try key.authMessage(server: "srv01", nonce: "n0nce", controlUrl: "wss://Hub.Example.com/lcp/v0/device")
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any])
        XCTAssertEqual(json["type"] as? String, "auth")
        let signature = try XCTUnwrap(Data(base64Encoded: XCTUnwrap(json["signature"] as? String)))
        let host = try XCTUnwrap(json["host"] as? String)
        let payload = Data("tallycam-lcp-auth-v2\nsrv01\nn0nce\n\(key.deviceId)\n\(host)".utf8)
        let publicKey = try P256.Signing.PublicKey(x963Representation: key.signer.publicKeyX963)
        XCTAssertTrue(publicKey.isValidSignature(try P256.Signing.ECDSASignature(rawRepresentation: signature),
                                                 for: payload))
        // The core's verifier (the server's) agrees.
        XCTAssertTrue(deviceKeyVerify(publicKeyX963: key.signer.publicKeyX963, payload: payload, signature: signature))
        XCTAssertFalse(deviceKeyVerify(publicKeyX963: key.signer.publicKeyX963,
                                       payload: Data("other".utf8), signature: signature))
        // A challenge that could smuggle a separator is not signed.
        XCTAssertThrowsError(try key.authMessage(server: "a\nb", nonce: "n", controlUrl: "ws://h/x"))
    }

    func testStoredKeyRoundTrips() throws {
        let signer = try DeviceKey.Signer.create(secureEnclave: false)
        let back = try DeviceKey.Signer(stored: signer.storedRepresentation)
        XCTAssertEqual(back.publicKeyX963, signer.publicKeyX963)
        XCTAssertThrowsError(try DeviceKey.Signer(stored: Data([9, 1, 2])))
    }

    func testChallengeIsParsed() {
        guard case let .challenge(server, nonce) = LcpIncoming.parse(#"{"type":"challenge","server":"s","nonce":"n"}"#)
        else {
            return XCTFail("not a challenge")
        }
        XCTAssertEqual(server, "s")
        XCTAssertEqual(nonce, "n")
        XCTAssertNil(LcpIncoming.parse(#"{"type":"challenge","server":"s"}"#))
    }
}
