import Foundation
import XCTest

final class LcpSeiTests: XCTestCase {
    private let uuid: [UInt8] = [
        0x8E, 0x3D, 0x91, 0xEF, 0x34, 0x90, 0x42, 0xBC, 0x90, 0xA2, 0x09, 0x7E, 0xEB, 0x07, 0xF1, 0xA6,
    ]

    /// RBSP of a core-built NAL unit (test-side emulation prevention removal).
    private func rbsp(_ nal: [UInt8], header: Int) -> [UInt8] {
        var out: [UInt8] = []
        var zeros = 0
        for byte in nal.dropFirst(header) {
            if zeros >= 2, byte == 0x03 {
                zeros = 0
                continue
            }
            out.append(byte)
            zeros = byte == 0 ? zeros + 1 : 0
        }
        return out
    }

    func testPayloadLayoutFromCore() {
        let nal = [UInt8](TimestampSei.nalUnit(captureNs: 0x1122_3344_5566_7788, synced: true,
                                               frameCounter: 0xA1B2_C3D4, uncertaintyUs: 900, codec: .h264))
        let body = rbsp(nal, header: 1)
        XCTAssertEqual(nal[0], 0x06)
        XCTAssertEqual(Array(body[0 ..< 2]), [0x05, 34], "user data unregistered, 34 bytes")
        let payload = Array(body[2 ..< 36])
        XCTAssertEqual(Array(payload[0 ..< 16]), uuid)
        XCTAssertEqual(payload[16], 1, "version")
        XCTAssertEqual(payload[17], 0x03, "flags: synced, TAI")
        XCTAssertEqual(Array(payload[18 ..< 26]), [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88])
        XCTAssertEqual(Array(payload[26 ..< 30]), [0xA1, 0xB2, 0xC3, 0xD4])
        XCTAssertEqual(Array(payload[30 ..< 34]), [0x00, 0x00, 0x03, 0x84])
        XCTAssertEqual(body.last, 0x80)

        let unsynced = rbsp([UInt8](TimestampSei.nalUnit(captureNs: 1, synced: false, frameCounter: 0,
                                                         uncertaintyUs: TimestampSei.uncertaintyUnknown,
                                                         codec: .h264)), header: 1)
        XCTAssertEqual(unsynced[2 + 17], 0x02, "TAI only")
        XCTAssertEqual(Array(unsynced[2 + 30 ..< 2 + 34]), [0xFF, 0xFF, 0xFF, 0xFF])
    }

    func testUncertaintyConversion() {
        XCTAssertEqual(TimestampSei.uncertaintyUs(fromNs: nil), 0xFFFF_FFFF)
        XCTAssertEqual(TimestampSei.uncertaintyUs(fromNs: 900_000), 900)
        XCTAssertEqual(TimestampSei.uncertaintyUs(fromNs: 900_001), 901, "rounds up")
        XCTAssertEqual(TimestampSei.uncertaintyUs(fromNs: Int64.max), 0xFFFF_FFFF, "saturates")
    }

    /// The bytes the earlier Swift implementation produced (UTC, no TAI
    /// flag): the core is byte-compatible with it.
    func testH264NalWithEmulationPrevention() {
        let nal = [UInt8](TimestampSei.nalUnit(captureNs: 0x0000_0001_0000_0000, synced: false, tai: false,
                                               frameCounter: 3, uncertaintyUs: 0x100, codec: .h264))
        let expected: [UInt8] = [0x06, 0x05, 34] + uuid + [
            0x01, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x01, 0x00, 0x00, 0x03, 0x00,
            0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x03, 0x00, 0x00, 0x03, 0x01, 0x00,
            0x80,
        ]
        XCTAssertEqual(nal, expected)
        assertNoStartCodeEmulation(nal)
    }

    func testEmulationPreventionAtEveryPosition() {
        for shift: UInt64 in 0 ..< 64 {
            let value: UInt64 = 1 << shift
            let nal = [UInt8](TimestampSei.nalUnit(captureNs: value, synced: shift & 1 == 1,
                                                   frameCounter: UInt32(truncatingIfNeeded: value),
                                                   uncertaintyUs: 0, codec: .h264))
            XCTAssertLessThanOrEqual(nal.count, TimestampSei.maxNalLength)
            assertNoStartCodeEmulation(nal)
            XCTAssertEqual(rbsp(nal, header: 1).count, 2 + 34 + 1)
        }
    }

    func testHevcPrefixSei() {
        let nal = [UInt8](TimestampSei.nalUnit(captureNs: 0x1122_3344_5566_7788, synced: true, frameCounter: 7,
                                               uncertaintyUs: 1, codec: .hevc))
        XCTAssertEqual(Array(nal[0 ..< 2]), [0x4E, 0x01], "prefix SEI, type 39, layer 0, tid 1")
        XCTAssertEqual((nal[0] >> 1) & 0x3F, 39)
        XCTAssertEqual(Array(rbsp(nal, header: 2)[0 ..< 2]), [0x05, 34])
    }

    // MARK: Helpers

    private func assertNoStartCodeEmulation(_ nal: [UInt8], file: StaticString = #filePath, line: UInt = #line) {
        guard nal.count >= 3 else {
            return
        }
        for i in 0 ..< nal.count - 2 where nal[i] == 0 && nal[i + 1] == 0 {
            XCTAssertGreaterThan(nal[i + 2], 0x02, "00 00 0\(nal[i + 2]) at \(i)", file: file, line: line)
        }
    }
}
