import XCTest

/// Hostile input on the control channel: deep nesting must not overflow
/// the stack, and numbers are numbers.
final class LcpParseTests: XCTestCase {
    func testDeepNestingIsDropped() {
        let deep = "{\"type\":\"set\",\"values\":{\"a\":" + String(repeating: "[", count: 300)
            + String(repeating: "]", count: 300) + "}}"
        XCTAssertNil(LcpIncoming.parse(deep))
        XCTAssertNil(LcpIncoming.depth(of: Data(deep.utf8), atMost: LcpIncoming.maxDepth))
    }

    func testBracketsInStringsDoNotCount() {
        let text = "{\"type\":\"message\",\"text\":\"" + String(repeating: "[{", count: 100) + "\\\"]\"}"
        XCTAssertEqual(LcpIncoming.depth(of: Data(text.utf8), atMost: LcpIncoming.maxDepth), 1)
        XCTAssertNotNil(LcpIncoming.parse(text))
    }

    func testOrdinaryNestingParses() {
        let text = "{\"type\":\"set\",\"id\":1,\"values\":{\"iso\":100,\"a\":[[1]]}}"
        XCTAssertEqual(LcpIncoming.depth(of: Data(text.utf8), atMost: LcpIncoming.maxDepth), 4)
        XCTAssertNotNil(LcpIncoming.parse(text))
    }

    func testStringsAreNotNumbers() {
        XCTAssertNil(JSONValue.string("nan").double)
        XCTAssertNil(JSONValue.string("30").double)
        XCTAssertEqual(JSONValue.number(30).double, 30)
    }
}
