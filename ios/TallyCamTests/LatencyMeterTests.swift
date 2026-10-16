import Foundation
import XCTest

final class LatencyMeterTests: XCTestCase {
    func testPercentilesPerWindow() {
        var meter = LatencyMeter()
        for ms in 1 ... 100 {
            meter.record(.encode, ns: Int64(ms) * 1_000_000)
        }
        meter.record(.mux, ns: -5)
        let report = meter.report()
        XCTAssertEqual(report.encodeMs, LatencyReport.Percentiles(p50: 51, p95: 95, max: 100))
        XCTAssertEqual(report.muxMs, LatencyReport.Percentiles(p50: 0, p95: 0, max: 0), "negative is 0")
        XCTAssertNil(report.sendQueueMs)
        // A new window starts after each report.
        XCTAssertNil(meter.report().encodeMs)
    }

    func testCapacityPerWindow() {
        var meter = LatencyMeter()
        for _ in 0 ..< LatencyMeter.capacity {
            meter.record(.captureToSend, ns: 1_000_000)
        }
        meter.record(.captureToSend, ns: 900_000_000)
        XCTAssertEqual(meter.report().captureToSendMs?.max, 1, "samples beyond the capacity are dropped")
    }

    func testReportJsonUsesSnakeCaseAndOmitsEmptyStages() throws {
        var meter = LatencyMeter()
        meter.record(.audioCaptureToMux, ns: 70_250_000)
        let json = try XCTUnwrap(LcpCoding.encode(meter.report()))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any])
        XCTAssertEqual(Array(object.keys), ["audio_capture_to_mux_ms"])
        let stage = try XCTUnwrap(object["audio_capture_to_mux_ms"] as? [String: Double])
        XCTAssertEqual(stage, ["p50": 70.3, "p95": 70.3, "max": 70.3])
    }

    func testAudioEncodeStageKey() throws {
        var meter = LatencyMeter()
        meter.record(.audioEncode, ns: 400_000)
        let json = try XCTUnwrap(LcpCoding.encode(meter.report()))
        XCTAssertTrue(json.contains("\"audio_encode_ms\""), json)
    }

    func testSrtLatencyIsSetInTheUrl() {
        XCTAssertEqual(SrtUrl.settingLatency(80, in: "srt://192.0.2.1:9000?streamid=cam-a&latency=500"),
                       "srt://192.0.2.1:9000?streamid=cam-a&latency=80")
        XCTAssertEqual(SrtUrl.settingLatency(120, in: "srt://192.0.2.1:9000?streamid=cam-a"),
                       "srt://192.0.2.1:9000?streamid=cam-a&latency=120")
        XCTAssertNil(SrtUrl.settingLatency(120, in: "http://example.com"))
    }
}
