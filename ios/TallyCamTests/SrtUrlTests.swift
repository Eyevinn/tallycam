import Foundation
import XCTest

/// The stream URL to the core's SRT caller options (`srtCallerOptionsFromUrl`)
/// and the URL edits the controls make.
final class SrtUrlTests: XCTestCase {
    func testPlainUrlGetsTheDefaults() throws {
        let url = try SrtUrl(string: " srt://192.0.2.10:9000?streamid=cam-3f2a9c1d ")
        XCTAssertEqual(url.host, "192.0.2.10")
        XCTAssertEqual(url.port, 9000)
        XCTAssertEqual(url.streamId, "cam-3f2a9c1d")
        XCTAssertEqual(url.latency, SrtUrl.defaultLatency)
        XCTAssertNil(url.options.passphrase)
        XCTAssertEqual(url.options.connectTimeoutMs, 5000)
        XCTAssertEqual(url.options.peerIdleTimeoutMs, 5000)
        XCTAssertEqual(url.options.maxBitrateBps, 50_000_000)
        XCTAssertFalse(url.options.reconnect)
    }

    func testEncryptionAndEscapes() throws {
        let url = try SrtUrl(string: "srt://hub.example.com:9100?streamid=%23!%3A%3Ar%3Dcam1&latency=80"
            + "&passphrase=secret%2Bphrase%26x&pbkeylen=32&mode=caller")
        XCTAssertEqual(url.host, "hub.example.com")
        XCTAssertEqual(url.streamId, "#!::r=cam1")
        XCTAssertEqual(url.latency, 80)
        XCTAssertEqual(url.options.passphrase, "secret+phrase&x")
        XCTAssertEqual(url.options.pbkeylen, 32)

        let aes128 = try SrtUrl(string: "srt://[2001:db8::1]:9100?passphrase=0123456789")
        XCTAssertEqual(aes128.host, "2001:db8::1")
        XCTAssertEqual(aes128.options.pbkeylen, 0, "0 = AES-128 with a passphrase")
    }

    func testRejectsWhatTheCallerCannotDo() {
        let cases = [
            "http://example.com:9000": "URL must start with srt://",
            "srt://example.com": "URL has no port",
            "srt://example.com:9000?latency=x": "latency must be 0-65535 ms",
            "srt://example.com:9000?passphrase=short": "passphrase must be 10-79 characters",
            "srt://example.com:9000?passphrase=0123456789&pbkeylen=20": "pbkeylen must be 16, 24 or 32",
            "srt://example.com:9000?mode=listener": "Only caller mode is supported",
        ]
        for (string, message) in cases {
            XCTAssertThrowsError(try SrtUrl(string: string), string) { error in
                XCTAssertEqual(error.localizedDescription, message, string)
            }
        }
    }

    func testSettingLatencyKeepsAnEscapedPassphrase() throws {
        let url = "srt://192.0.2.1:9000?streamid=cam-a&passphrase=secret%2Bphrase&pbkeylen=16&latency=500"
        let edited = try XCTUnwrap(SrtUrl.settingLatency(100, in: url))
        XCTAssertEqual(edited, "srt://192.0.2.1:9000?streamid=cam-a&passphrase=secret%2Bphrase&pbkeylen=16&latency=100")
        let parsed = try SrtUrl(string: edited)
        XCTAssertEqual(parsed.options.passphrase, "secret+phrase")
        XCTAssertEqual(parsed.latency, 100)
    }

    func testRedactedHidesOnlyThePassphrase() {
        XCTAssertEqual(SrtUrl.redacted("srt://192.0.2.1:9000?streamid=cam-a&passphrase=0123456789&pbkeylen=32"),
                       "srt://192.0.2.1:9000?streamid=cam-a&passphrase=***&pbkeylen=32")
        XCTAssertEqual(SrtUrl.redacted("srt://192.0.2.1:9000?streamid=cam-a"), "srt://192.0.2.1:9000?streamid=cam-a")
        XCTAssertEqual(SrtUrl.redacted(""), "")
    }
}
