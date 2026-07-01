import XCTest

/// B1P acceptance: launching with `-OBCScenario X` boots directly into that
/// state, and the dev panel is reachable. The app under test is a Debug build,
/// so the `MockStatusHUD` tags (`mockScenarioTag` / `mockConnectionTag`) are
/// present; screen-level assertions arrive with the real screens (B2+), which
/// reuse `launch(arguments:)` below.
final class ScenarioLaunchTests: XCTestCase {
    /// Keep in sync with `Scenario.allCases` (OBCMock/Scenario.swift). The
    /// token↔case round-trip is host-tested in `MockLaunchOptionsTests`; this
    /// list is what proves each token boots the app end to end.
    private static let scenarios = [
        "happyPath", "emptyLibrary", "coldRead", "readError", "outOfRange",
        "noDevice", "pairingTimeout", "pairingRejected", "bluetoothOff",
        "permissionDenied", "syncUpToDate", "syncDrop", "uploadDrop",
        "unsupportedFile",
    ]

    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch(arguments: [String]) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += arguments
        app.launch()
        return app
    }

    /// One smoke check per scenario: the app launches into it and the HUD
    /// reports it. Grouped as activities so a failure names its scenario.
    @MainActor
    func testEveryScenarioBootsViaLaunchArg() {
        for scenario in Self.scenarios {
            XCTContext.runActivity(named: scenario) { _ in
                let app = launch(arguments: ["-OBCScenario", scenario])
                let tag = app.staticTexts["mockScenarioTag"]
                XCTAssertTrue(tag.waitForExistence(timeout: 10), "\(scenario): HUD tag missing")
                XCTAssertEqual(tag.label, scenario)
                app.terminate()
            }
        }
    }

    /// `-OBCConnection` overrides the scenario's initial link state.
    @MainActor
    func testConnectionOverrideAppliesOnTopOfScenario() {
        let app = launch(arguments: ["-OBCScenario", "happyPath", "-OBCConnection", "outOfRange"])
        let tag = app.staticTexts["mockConnectionTag"]
        XCTAssertTrue(tag.waitForExistence(timeout: 10))
        XCTAssertEqual(tag.label, "outOfRange")
    }

    /// The booted state actually drives the transport: the fixture device name
    /// flows through `MockTransport.deviceInfo()` into the UI.
    @MainActor
    func testHappyPathServesTheFixtureDevice() {
        let app = launch(arguments: ["-OBCScenario", "happyPath"])
        XCTAssertTrue(app.staticTexts["Trailhead"].waitForExistence(timeout: 10))
    }

    /// The dev panel presents at launch via `-OBCShowDevPanel` and dismisses.
    @MainActor
    func testDevPanelOpensViaLaunchArgAndDismisses() {
        let app = launch(arguments: ["-OBCShowDevPanel"])
        let panel = app.otherElements["devPanel"].firstMatch
        let done = app.buttons["devPanel.done"]
        XCTAssertTrue(done.waitForExistence(timeout: 10), "dev panel did not present")
        XCTAssertTrue(panel.exists || app.collectionViews.count > 0)
        done.tap()
        XCTAssertTrue(app.staticTexts["mockScenarioTag"].waitForExistence(timeout: 5))
        XCTAssertFalse(done.exists)
    }
}
