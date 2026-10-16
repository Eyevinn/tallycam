import Foundation
import XCTest

/// The Swift side of the core clock: host clock, the lock-free mapping and
/// the `status.clock` / `clock_log` messages. The discipline itself is
/// tested in Rust (`cargo test`, core/tests/vectors).
final class ClockCoreTests: XCTestCase {
    func testHostClockMatchesUptime() {
        let host = Double(HostClock.nowNs())
        let uptime = ProcessInfo.processInfo.systemUptime * 1e9
        XCTAssertEqual(host, uptime, accuracy: 50_000_000)
        XCTAssertEqual(HostClock.ns(fromTicks: 0), 0)
    }

    /// Before the clock service runs, frames get the device wall clock as
    /// TAI (UTC + 37 s), not synced, unknown error.
    func testMappingFallsBackToWallClockAsTai() {
        let now = HostClock.nowNs()
        let mapping = ClockMapping.at(hostNs: now)
        XCTAssertFalse(mapping.synced)
        XCTAssertNil(mapping.maxErrorNs)
        XCTAssertEqual(mapping.taiNs - mapping.utcNs, 37_000_000_000, "TAI - UTC as of 2026")
        XCTAssertEqual(Double(mapping.utcNs), Double(HostClock.wallNowNs()), accuracy: 50_000_000)
        let earlier = ClockMapping.at(hostNs: now - 1_000_000_000)
        XCTAssertEqual(Double(mapping.taiNs - earlier.taiNs), 1e9, accuracy: 50_000_000)
    }

    func testMappingCostsNoAllocation() {
        // Per frame and per audio buffer: a million mappings are quick.
        let start = HostClock.nowNs()
        var sum: Int64 = 0
        for i in 0 ..< 1_000_000 {
            sum &+= ClockMapping.at(hostNs: start + Int64(i)).taiNs
        }
        XCTAssertNotEqual(sum, 0)
        let perCallNs = Double(HostClock.nowNs() - start) / 1e6
        XCTAssertLessThan(perCallNs, 2000)
    }

    func testStatusCarriesClockObject() throws {
        let clock = try JSONDecoder().decode(JSONValue.self, from: Data("""
        {"synced":true,"health":"healthy","tai_utc_s":37,"max_error_us":4785,
         "sources":[{"state":"*","name":"ntp.ubuntu.com","reach":255}]}
        """.utf8))
        let status = StatusMessage(battery: nil, thermal: "nominal",
                                   stream: StatusMessage.Stream(state: "idle", url: "srt://192.0.2.10:9000"),
                                   network: "wifi", clock: clock)
        let text = try XCTUnwrap(LcpCoding.encode(status))
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any])
        let encoded = try XCTUnwrap(json["clock"] as? [String: Any])
        XCTAssertEqual(encoded["tai_utc_s"] as? Int, 37)
        XCTAssertEqual(encoded["max_error_us"] as? Int, 4785)
        XCTAssertNil(json["time"], "status.time is gone")
    }

    func testResultCarriesData() throws {
        let result = ResultMessage(id: .int(4), ok: true, data: .object(["lines": .array([.string("event x")])]))
        let text = try XCTUnwrap(LcpCoding.encode(result))
        XCTAssertTrue(text.contains(#""data":{"lines":["event x"]}"#), text)
    }

    func testTimeResponseIsNoLongerParsed() {
        XCTAssertNil(LcpIncoming.parse(#"{"type":"time_response","id":1,"t1":1,"t2":2,"t3":3}"#))
    }
}
