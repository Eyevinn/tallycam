import XCTest

final class ProvisioningTests: XCTestCase {
    private let deviceId = "3F2A9C1D-0B7E-4E55-9A10-2C3D4E5F6A7B"

    /// The link the hub serves (percent-encoded with RFC 3986 unreserved set).
    private let hubLink = "tallycam://connect?control=ws%3A%2F%2F192.0.2.10%3A8090%2Flcp%2Fv0%2Fdevice"
        + "&srt=srt%3A%2F%2F192.0.2.10%3A9000%3Fstreamid%3D%7Bdevice%7D%26latency%3D500"

    func testParsesHubLinkAndReplacesDevice() throws {
        let link = try XCTUnwrap(ProvisioningLink(url: XCTUnwrap(URL(string: hubLink)), deviceId: deviceId))
        XCTAssertEqual(link.controlUrl, "ws://192.0.2.10:8090/lcp/v0/device")
        XCTAssertEqual(link.srtUrl, "srt://192.0.2.10:9000?streamid=cam-3f2a9c1d&latency=500")
        XCTAssertEqual(link.controlHost, "192.0.2.10")
    }

    func testFixedStreamIdIsKept() throws {
        let url = try XCTUnwrap(URL(string: "tallycam://connect?control=wss%3A%2F%2Fhub.example.com%2Flcp%2Fv0%2Fdevice"
                + "&srt=srt%3A%2F%2Fhub.example.com%3A9000%3Fstreamid%3Dstage-left"))
        let link = try XCTUnwrap(ProvisioningLink(url: url, deviceId: deviceId))
        XCTAssertEqual(link.controlUrl, "wss://hub.example.com/lcp/v0/device")
        XCTAssertEqual(link.srtUrl, "srt://hub.example.com:9000?streamid=stage-left")
    }

    func testSrtIsOptional() throws {
        let url = try XCTUnwrap(URL(string: "tallycam://connect?control=ws%3A%2F%2F192.0.2.10%3A8090%2Fx"))
        XCTAssertNil(try XCTUnwrap(ProvisioningLink(url: url, deviceId: deviceId)).srtUrl)
    }

    func testRejectsOtherLinks() {
        let bad = [
            "https://example.com/connect?control=ws%3A%2F%2Fh%2Fx",
            "tallycam://other?control=ws%3A%2F%2Fh%2Fx",
            "tallycam://connect",
            "tallycam://connect?control=http%3A%2F%2Fh%2Fx",
            "tallycam://connect?control=ws%3A%2F%2Fh%2Fx&srt=rtmp%3A%2F%2Fh%2Fx",
        ]
        for string in bad {
            XCTAssertNil(ProvisioningLink(url: URL(string: string)!, deviceId: deviceId), string)
        }
    }

    func testScannedLink() throws {
        let scanned = try XCTUnwrap(ProvisioningLink.scannedLink(" \(hubLink)\n"))
        XCTAssertNotNil(ProvisioningLink(url: scanned, deviceId: deviceId))
        XCTAssertNotNil(ProvisioningLink.scannedLink("TALLYCAM://connect"))
        XCTAssertNil(ProvisioningLink.scannedLink("https://example.com"))
        XCTAssertNil(ProvisioningLink.scannedLink("WIFI:S:venue;T:WPA;P:secret;;"))
    }

    func testStreamId() {
        XCTAssertEqual(ProvisioningLink.streamId(deviceId: deviceId), "cam-3f2a9c1d")
        XCTAssertEqual(ProvisioningLink.streamId(deviceId: "ab"), "cam-ab")
    }

    func testSameServerJoinsWithoutAsking() {
        let current = "ws://192.0.2.10:8090/lcp/v0/device"
        XCTAssertTrue(ProvisioningLink.sameServer("ws://192.0.2.10:8090/lcp/v0/device", current))
        // Another path or token on the same server is the same hub.
        XCTAssertTrue(ProvisioningLink.sameServer("ws://192.0.2.10:8090/other?token=abc", current))
        XCTAssertTrue(ProvisioningLink.sameServer("WS://HUB.example.com/x", "ws://hub.example.com:80/y"))
        XCTAssertTrue(ProvisioningLink.sameServer("wss://hub.example.com/x", " wss://hub.example.com:443/y "))
    }

    func testOtherServerOrNoneAsks() {
        let current = "ws://192.0.2.10:8090/lcp/v0/device"
        XCTAssertFalse(ProvisioningLink.sameServer("ws://192.0.2.11:8090/lcp/v0/device", current))
        XCTAssertFalse(ProvisioningLink.sameServer("ws://192.0.2.10:8091/lcp/v0/device", current))
        XCTAssertFalse(ProvisioningLink.sameServer("ws://hub.example.com/x", "wss://hub.example.com/x"))
        XCTAssertFalse(ProvisioningLink.sameServer(current, ""))
        XCTAssertFalse(ProvisioningLink.sameServer(current, "   "))
        XCTAssertFalse(ProvisioningLink.sameServer(current, "not a url"))
    }

    func testJoinMessage() throws {
        let link = try XCTUnwrap(ProvisioningLink(url: XCTUnwrap(URL(string: hubLink)), deviceId: deviceId))
        XCTAssertEqual(link.joinTitle, "Join 192.0.2.10?")
        XCTAssertEqual(link.joinMessage(live: false),
                       "Control hub: 192.0.2.10:8090\nStream to: 192.0.2.10:9000\nNot encrypted (ws://)")
        XCTAssertEqual(link.joinMessage(live: true),
                       "Control hub: 192.0.2.10:8090\nStream to: 192.0.2.10:9000\nThe stream URL applies on the next Go Live"
                           + "\nNot encrypted (ws://)")
        let noSrt = try XCTUnwrap(ProvisioningLink(url: XCTUnwrap(URL(string: "tallycam://connect?control=wss%3A%2F%2Fhub.example.com%2Fx")),
                                                   deviceId: deviceId))
        XCTAssertEqual(noSrt.joinMessage(live: true), "Control hub: hub.example.com\nStream: unchanged")
    }

    private func link(_ control: String, srt: String? = nil) throws -> ProvisioningLink {
        let encode = { (s: String) in s.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? s }
        let text = "tallycam://connect?control=\(encode(control))" + (srt.map { "&srt=\(encode($0))" } ?? "")
        return try XCTUnwrap(ProvisioningLink(url: XCTUnwrap(URL(string: text)), deviceId: deviceId))
    }

    /// The same hub with the same (or no) stream target joins silently;
    /// another path, scheme or SRT target asks (a QR with the real hub and an
    /// attacker's SRT must not redirect the stream silently).
    func testJoinsSilentlyOnlyWhenNothingChanges() throws {
        let control = "wss://hub.example.com/lcp/v0/device"
        let srt = "srt://hub.example.com:9000?streamid=cam-a"
        XCTAssertTrue(try link(control).joinsSilently(currentControl: control, currentSrt: srt))
        XCTAssertTrue(try link("wss://HUB.example.com:443/lcp/v0/device", srt: "srt://hub.example.com:9000?streamid=x")
            .joinsSilently(currentControl: control, currentSrt: srt))
        XCTAssertFalse(try link(control, srt: "srt://evil.example.com:9000?streamid=cam-a")
            .joinsSilently(currentControl: control, currentSrt: srt))
        XCTAssertFalse(try link(control, srt: "srt://hub.example.com:9001")
            .joinsSilently(currentControl: control, currentSrt: srt))
        XCTAssertFalse(try link("wss://hub.example.com/other").joinsSilently(currentControl: control, currentSrt: srt))
        XCTAssertFalse(try link("ws://hub.example.com:443/lcp/v0/device")
            .joinsSilently(currentControl: control, currentSrt: srt))
        XCTAssertFalse(try link(control).joinsSilently(currentControl: "", currentSrt: srt))
    }
}
