import XCTest

final class CameraFormatsTests: XCTestCase {
    private func format(_ width: Int32, _ height: Int32, max: Double, min: Double = 1, binned: Bool = false,
                        videoRange: Bool = true, eightBit: Bool = true) -> CameraFormatInfo
    {
        CameraFormatInfo(width: width, height: height, is8Bit420: eightBit, isVideoRange: videoRange,
                         isBinned: binned, rateRanges: [.init(min: min, max: max)])
    }

    /// Shaped like a non-Pro iPhone wide lens: unbinned 1080p and 4K up to
    /// 60, binned 1080p up to 120 and 240, 10-bit (HDR) duplicates, a 4:3
    /// format and 720p.
    private var phone: [CameraFormatInfo] {
        [
            format(1280, 720, max: 60),
            format(1920, 1080, max: 60, videoRange: false),
            format(1920, 1080, max: 60),
            format(1920, 1080, max: 60, eightBit: false),
            format(1920, 1080, max: 120, binned: true),
            format(1920, 1080, max: 240, binned: true),
            format(1920, 1440, max: 60),
            format(3840, 2160, max: 30),
            format(3840, 2160, max: 60),
            format(3840, 2160, max: 60, eightBit: false),
        ]
    }

    func testResolutionsFromTheFormats() {
        XCTAssertEqual(CameraFormats.resolutions(phone), [.p720, .p1080, .p2160])
        XCTAssertEqual(CameraFormats.resolutions([format(1920, 1440, max: 30), format(3840, 2160, max: 30, eightBit: false)]),
                       [], "no 16:9 8-bit format")
    }

    func testFrameRatesPerResolution() {
        let hd = CameraFormats.frameRates(phone, resolution: .p1080)
        XCTAssertEqual(hd.map(\.frameRate), [24, 25, 30, 50, 60, 100, 120, 240])
        XCTAssertEqual(hd.filter(\.binned).map(\.frameRate), [100, 120, 240])
        XCTAssertEqual(CameraFormats.frameRates(phone, resolution: .p2160).map(\.frameRate), [24, 25, 30, 50, 60])
        XCTAssertEqual(CameraFormats.frameRates(phone, resolution: .p720).map(\.frameRate), [24, 25, 30, 50, 60])
    }

    func testBestPrefersUnbinnedThenLowestMaximumThenVideoRange() {
        let formats = phone
        XCTAssertEqual(CameraFormats.best(formats, resolution: .p1080, frameRate: 30), 2, "unbinned 420v")
        XCTAssertEqual(CameraFormats.best(formats, resolution: .p1080, frameRate: 120), 4, "binned 120 before 240")
        XCTAssertEqual(CameraFormats.best(formats, resolution: .p1080, frameRate: 240), 5)
        XCTAssertEqual(CameraFormats.best(formats, resolution: .p2160, frameRate: 30), 7, "the 30 fps format")
        XCTAssertEqual(CameraFormats.best(formats, resolution: .p2160, frameRate: 60), 8, "8-bit, not HDR")
        XCTAssertNil(CameraFormats.best(formats, resolution: .p2160, frameRate: 120))
        // A binned format with a lower maximum still loses to an unbinned one.
        XCTAssertEqual(CameraFormats.best([format(1920, 1080, max: 30, binned: true), format(1920, 1080, max: 60)],
                                          resolution: .p1080, frameRate: 30), 1)
    }

    func testFrameRateRangesAreInclusive() {
        let f = format(1920, 1080, max: 29.97, min: 2)
        XCTAssertFalse(f.supports(frameRate: 30), "AVFoundation refuses 1/30 s in a 29.97 range")
        XCTAssertTrue(f.supports(frameRate: 25))
        XCTAssertTrue(f.supports(frameRate: 2))
        XCTAssertFalse(f.supports(frameRate: 1))
        XCTAssertFalse(f.supports(frameRate: 50))
    }

    func testFallbacks() {
        XCTAssertEqual(CameraFormats.fallback(.p2160, available: [.p720, .p1080, .p2160]), .p2160)
        XCTAssertEqual(CameraFormats.fallback(.p2160, available: [.p720, .p1080]), .p1080, "largest below")
        XCTAssertEqual(CameraFormats.fallback(.p720, available: [.p1080, .p2160]), .p1080, "smallest above")
        XCTAssertNil(CameraFormats.fallback(.p1080, available: []))
        XCTAssertEqual(CameraFormats.fallback(frameRate: 50, available: [24, 25, 30, 60]), 30)
        XCTAssertEqual(CameraFormats.fallback(frameRate: 240, available: [24, 25, 30, 60]), 60)
        XCTAssertEqual(CameraFormats.fallback(frameRate: 10, available: [24, 30]), 24)
    }

    func testResolutionIds() {
        XCTAssertEqual(VideoResolution.allCases.map(\.rawValue), ["720p", "1080p", "2160p"])
        XCTAssertEqual(VideoResolution(width: 3840, height: 2160), .p2160)
        XCTAssertNil(VideoResolution(width: 1920, height: 1440))
    }

    /// The bitrate policy is in the core (core/src/bitrate.rs, tested there);
    /// this checks the mapping.
    func testBitrateFromTheCore() {
        let r = VideoResolution.p1080
        XCTAssertEqual(VideoBitrate.defaultKbps(width: r.width, height: r.height, frameRate: 30, codec: .h264), 6000)
        XCTAssertEqual(VideoBitrate.defaultKbps(width: r.width, height: r.height, frameRate: 30, codec: .hevc), 4000)
        XCTAssertEqual(VideoBitrate.maxKbps(width: r.width, height: r.height, frameRate: 30, codec: .h264), 20000)
        XCTAssertEqual(VideoBitrate.maxKbps(width: 3840, height: 2160, frameRate: 60, codec: .hevc), 45000)
        XCTAssertEqual(VideoBitrate.minKbps, 1000)
    }
}
