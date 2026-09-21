import XCTest

/// One smoke test, so the target builds and the app launches under XCUITest. The scenario-driven
/// suites live beside this file.
final class OBCCompanionUITests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    func testAppLaunches() {
        let app = XCUIApplication()
        app.launch()
        XCTAssertEqual(app.state, .runningForeground)
    }
}
