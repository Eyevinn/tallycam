import XCTest

final class ClockSummaryTests: XCTestCase {
    func testParsesStatusClock() throws {
        let json = """
        { "synced": true, "health": "degraded", "findings": ["max error 600 ms"],
          "est_error_us": 212, "max_error_us": 3162, "reference": "192.0.2.21",
          "reference_name": "1.ubuntu.pool.ntp.org", "stratum": 3, "reachable": 4, "total": 4 }
        """
        let value = try JSONDecoder().decode(JSONValue.self, from: Data(json.utf8))
        let clock = ClockSummary(status: value)
        XCTAssertTrue(clock.synced)
        XCTAssertEqual(clock.health, .degraded)
        XCTAssertEqual(clock.findings, ["max error 600 ms"])
        XCTAssertEqual(clock.estErrorUs, 212)
        XCTAssertEqual(clock.referenceName, "1.ubuntu.pool.ntp.org")
        XCTAssertEqual(clock.stratum, 3)
        XCTAssertEqual(clock.reachable, 4)
    }

    func testNotStartedIsBad() {
        XCTAssertEqual(ClockSummary(status: .object(["synced": .bool(false)])).health, .bad)
    }

    func testFormat() {
        XCTAssertEqual(ClockSummary.formatMicroseconds(212), "212 µs")
        XCTAssertEqual(ClockSummary.formatMicroseconds(3162), "3.2 ms")
        XCTAssertEqual(ClockSummary.formatMicroseconds(1_200_000), "1.20 s")
        XCTAssertEqual(ClockSummary.formatMicroseconds(nil), "-")
    }
}
