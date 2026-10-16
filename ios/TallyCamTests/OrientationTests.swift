import CoreGraphics
import XCTest

final class OrientationTests: XCTestCase {
    private func resolve(_ mode: OrientationMode, _ side: DeviceSide, last: DeviceSide = .landscapeHomeRight,
                         locked: Bool? = nil) -> StreamOrientation
    {
        StreamOrientation.resolve(mode: mode, side: side, lastLandscape: last, lockedPortrait: locked)
    }

    func testAutoFollowsTheDevice() {
        XCTAssertEqual(resolve(.auto, .portrait), .portrait)
        XCTAssertEqual(resolve(.auto, .landscapeHomeRight), .landscapeHomeRight)
        XCTAssertEqual(resolve(.auto, .landscapeHomeLeft), .landscapeHomeLeft)
        XCTAssertEqual(resolve(.auto, .portraitUpsideDown), .portraitUpsideDown, "iPad")
    }

    func testFixedModes() {
        XCTAssertEqual(resolve(.landscape, .portrait), .landscapeHomeRight)
        XCTAssertEqual(resolve(.landscape, .portrait, last: .landscapeHomeLeft), .landscapeHomeLeft)
        XCTAssertEqual(resolve(.landscape, .landscapeHomeLeft), .landscapeHomeLeft, "upright on both sides")
        XCTAssertEqual(resolve(.portrait, .landscapeHomeRight), .portrait)
        XCTAssertEqual(resolve(.portrait, .portrait), .portrait)
    }

    /// While live the geometry stays; only the upright side follows.
    func testLockedGeometry() {
        XCTAssertEqual(resolve(.auto, .portrait, locked: false), .landscapeHomeRight)
        XCTAssertEqual(resolve(.auto, .landscapeHomeLeft, locked: false), .landscapeHomeLeft)
        XCTAssertEqual(resolve(.auto, .landscapeHomeLeft, locked: true), .portrait)
        XCTAssertEqual(resolve(.portrait, .landscapeHomeRight, locked: false), .landscapeHomeRight)
        XCTAssertEqual(resolve(.landscape, .portrait, locked: true), .portrait)
    }

    func testSizesAndAngles() {
        let hd = (width: Int32(1920), height: Int32(1080))
        XCTAssertEqual(StreamOrientation.landscapeHomeRight.size(landscape: hd).width, 1920)
        XCTAssertEqual(StreamOrientation.landscapeHomeRight.size(landscape: hd).height, 1080)
        XCTAssertEqual(StreamOrientation.portrait.size(landscape: hd).width, 1080)
        XCTAssertEqual(StreamOrientation.portrait.size(landscape: hd).height, 1920)
        XCTAssertEqual(StreamOrientation.landscapeHomeLeft.size(landscape: hd).width, 1920)
        let uhd = (width: Int32(3840), height: Int32(2160))
        XCTAssertEqual(StreamOrientation.portraitUpsideDown.size(landscape: uhd).width, 2160)
        XCTAssertEqual(StreamOrientation.portraitUpsideDown.size(landscape: uhd).height, 3840)
        XCTAssertEqual(StreamOrientation.landscapeHomeLeft.size(landscape: uhd).width, 3840)
        XCTAssertEqual(StreamOrientation.landscapeHomeRight.rotationAngle, 0)
        XCTAssertEqual(StreamOrientation.portrait.rotationAngle, 90)
        XCTAssertEqual(StreamOrientation.landscapeHomeLeft.rotationAngle, 180)
        XCTAssertEqual(StreamOrientation.portraitUpsideDown.rotationAngle, 270)
    }

    func testModeCycle() {
        XCTAssertEqual(OrientationMode.auto.next, .landscape)
        XCTAssertEqual(OrientationMode.landscape.next, .portrait)
        XCTAssertEqual(OrientationMode.portrait.next, .auto)
    }

    /// The sensor image rotated clockwise by `angle` (what the capture
    /// connection does), applied to a normalized sensor point.
    private func streamed(_ sensor: CGPoint, angle: CGFloat) -> CGPoint {
        var point = sensor
        for _ in 0 ..< Int(angle / 90) {
            point = CGPoint(x: 1 - point.y, y: point.x)
        }
        return point
    }

    func testFocusPointMapsBackToTheSensor() {
        let points = [CGPoint(x: 0.25, y: 0.75), CGPoint(x: 0, y: 0), CGPoint(x: 1, y: 0.1), CGPoint(x: 0.5, y: 0.5)]
        for angle: CGFloat in [0, 90, 180, 270] {
            for sensor in points {
                let shown = streamed(sensor, angle: angle)
                let back = StreamOrientation.devicePoint(x: shown.x, y: shown.y, angle: angle)
                XCTAssertEqual(back.x, sensor.x, accuracy: 1e-9, "angle \(angle)")
                XCTAssertEqual(back.y, sensor.y, accuracy: 1e-9, "angle \(angle)")
            }
        }
        // Portrait: the top left of the stream is the sensor's bottom left.
        XCTAssertEqual(StreamOrientation.devicePoint(x: 0, y: 0, angle: 90), CGPoint(x: 0, y: 1))
        // Out of range input is clamped.
        XCTAssertEqual(StreamOrientation.devicePoint(x: -1, y: 2, angle: 0), CGPoint(x: 0, y: 1))
    }
}

final class PreviewGeometryTests: XCTestCase {
    /// Landscape 16:9 frame in a wider view: pillarboxed.
    func testTapMapsThroughTheFittedRect() throws {
        let view = CGSize(width: 2000, height: 900)
        let frame = CGSize(width: 1920, height: 1080)
        let rect = PreviewGeometry.videoRect(viewSize: view, frameSize: frame)
        XCTAssertEqual(rect.height, 900, accuracy: 0.001)
        XCTAssertEqual(rect.width, 1600, accuracy: 0.001)
        XCTAssertEqual(rect.minX, 200, accuracy: 0.001)
        let centre = try XCTUnwrap(PreviewGeometry.streamPoint(tap: CGPoint(x: 1000, y: 450), viewSize: view, frameSize: frame))
        XCTAssertEqual(centre.x, 0.5, accuracy: 0.001)
        XCTAssertEqual(centre.y, 0.5, accuracy: 0.001)
        let corner = try XCTUnwrap(PreviewGeometry.streamPoint(tap: CGPoint(x: 200, y: 0), viewSize: view, frameSize: frame))
        XCTAssertEqual(corner.x, 0, accuracy: 0.001)
        XCTAssertNil(PreviewGeometry.streamPoint(tap: CGPoint(x: 100, y: 450), viewSize: view, frameSize: frame),
                     "outside the picture")
    }

    /// Portrait frame in a landscape view (locked portrait stream).
    func testPortraitFrameInLandscapeView() throws {
        let point = try XCTUnwrap(PreviewGeometry.streamPoint(tap: CGPoint(x: 500, y: 225),
                                                              viewSize: CGSize(width: 1000, height: 900),
                                                              frameSize: CGSize(width: 1080, height: 1920)))
        XCTAssertEqual(point.x, 0.5, accuracy: 0.001)
        XCTAssertEqual(point.y, 0.25, accuracy: 0.001)
    }
}
