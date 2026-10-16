import XCTest

final class CameraNameTests: XCTestCase {
    func testDefaultIsTheModelAndTheStartOfTheId() {
        XCTAssertEqual(CameraName.display(custom: nil, model: "iPhone17,3", deviceId: "55DD2703abcdef"), "iPhone 16 · 55dd")
        XCTAssertEqual(CameraName.display(custom: "  ", model: "iPad13,18", deviceId: "0b9fa55a"), "iPad 10 · 0b9f")
        // An unknown model shows its id; no device id yet, the model alone.
        XCTAssertEqual(CameraName.display(custom: nil, model: "iPhone99,1", deviceId: "abcd1234"), "iPhone99,1 · abcd")
        XCTAssertEqual(CameraName.display(custom: nil, model: "iPhone17,3", deviceId: nil), "iPhone 16")
    }

    func testCustomNameIsTrimmedAndCut() {
        XCTAssertEqual(CameraName.display(custom: "  Cam 2  ", model: "iPhone17,3", deviceId: "55dd"), "Cam 2")
        XCTAssertNil(CameraName.normalized(" \n "))
        let long = String(repeating: "x", count: 100)
        XCTAssertEqual(CameraName.normalized(long)?.count, CameraName.maxLength)
    }
}
