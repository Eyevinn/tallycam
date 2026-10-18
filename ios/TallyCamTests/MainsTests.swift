import XCTest

/// The mains rule (docs/SHADING.md): the last write wins, a phone change is
/// saved, a hub change holds for the session only.
final class MainsTests: XCTestCase {
    func testPhoneChangeIsSavedAndHoldsOverTheProduction() {
        var mains = Mains(own: 60)
        mains.production(50, changed: true)
        XCTAssertEqual(mains.effective, 50)
        // Equal to what applies, still saved as the phone's own.
        XCTAssertTrue(mains.set(50, fromHub: false))
        XCTAssertEqual(mains.own, 50)
        mains.leaveHub()
        XCTAssertEqual(mains.effective, 50)
    }

    func testHubChangeIsNeverSaved() {
        var mains = Mains(own: 60)
        mains.production(50, changed: true)
        XCTAssertFalse(mains.set(0, fromHub: true))
        XCTAssertEqual(mains.effective, 0)
        XCTAssertEqual(mains.own, 60)
        mains.leaveHub()
        XCTAssertEqual(mains.effective, 60)
    }

    func testANewProductionClearsTheHeldChangeTheSameOneKeepsIt() {
        var mains = Mains(own: 60)
        mains.production(50, changed: true)
        mains.set(0, fromHub: false)
        mains.production(50, changed: false)
        XCTAssertEqual(mains.effective, 0)
        mains.production(60, changed: true)
        XCTAssertEqual(mains.effective, 60)
    }

    func testHubChangeWithoutAProductionEndsWithTheHub() {
        var mains = Mains(own: 50)
        mains.set(60, fromHub: true)
        XCTAssertEqual(mains.effective, 60)
        mains.leaveHub()
        XCTAssertEqual(mains.effective, 50)
    }
}
