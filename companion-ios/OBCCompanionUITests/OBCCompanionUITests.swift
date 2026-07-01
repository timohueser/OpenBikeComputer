import XCTest

/// Empty XCUITest home for B1P (launch-arg / scenario-driven UI tests). B0 ships
/// one smoke test so the target builds and the app launches under XCUITest; B1P
/// wires the `-OBCScenario` / `-OBCConnection` launch args and the screen table.
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
