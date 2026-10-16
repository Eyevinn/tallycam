import AVFoundation
import XCTest

/// A `set` with both ISO and shutter must apply both in one call
/// (previously the shutter call put back the old ISO).
final class ManualExposureTests: XCTestCase {
    func testBothValuesAreCommittedTogether() {
        let duration = CMTime(seconds: 1.0 / 60, preferredTimescale: 1_000_000)
        let request = ManualExposureRequest(iso: 1600, duration: duration)
        let target = request.resolved()
        XCTAssertEqual(target.iso, 1600)
        XCTAssertEqual(CMTimeCompare(target.duration, duration), 0)
        XCTAssertEqual(request.stateValues(), ["iso": .number(1600), "shutter": .int(60)])
    }

    func testMissingParameterKeepsTheCurrentValue() {
        let isoOnly = ManualExposureRequest(iso: 100).resolved()
        XCTAssertEqual(isoOnly.iso, 100)
        XCTAssertEqual(CMTimeCompare(isoOnly.duration, AVCaptureDevice.currentExposureDuration), 0)
        let duration = CMTime(seconds: 1.0 / 50, preferredTimescale: 1_000_000)
        let shutterOnly = ManualExposureRequest(duration: duration).resolved()
        XCTAssertEqual(shutterOnly.iso, AVCaptureDevice.currentISO)
        XCTAssertEqual(ManualExposureRequest(duration: duration).stateValues(), ["shutter": .int(50)])
    }

    func testEmpty() {
        XCTAssertTrue(ManualExposureRequest().isEmpty)
        XCTAssertFalse(ManualExposureRequest(iso: 100).isEmpty)
    }
}
