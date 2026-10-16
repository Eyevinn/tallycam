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

    func testStreamId() {
        XCTAssertEqual(ProvisioningLink.streamId(deviceId: deviceId), "cam-3f2a9c1d")
        XCTAssertEqual(ProvisioningLink.streamId(deviceId: "ab"), "cam-ab")
    }
}
