import XCTest

/// Each pairing scenario drives its screens end to end through the real UI. The same branches are
/// host-tested against the state machine in `LaunchFlowModelTests`; this proves the wiring from
/// launch argument to scenario to screens.
final class PairingFlowTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch(scenario: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", scenario]
        app.launch()
        return app
    }

    /// Keep a named screenshot in the result bundle: the visual record of each design screen.
    @MainActor
    private func snap(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    /// The intro, the scan with the row sliding in, the pairing beat, then the main screen.
    @MainActor
    func testFirstRunPairingHappyPath() {
        let app = launch(scenario: "noDevice")

        XCTAssertTrue(app.staticTexts["pair.introTitle"].waitForExistence(timeout: 10), "D1 missing")
        snap(app, "D1-pairing-prompt")
        app.buttons["pair.start"].tap()

        let row = app.buttons["pair.deviceRow"]
        XCTAssertTrue(row.waitForExistence(timeout: 10), "D2 discovered row missing")
        snap(app, "D2-scanning-found")
        row.tap()

        XCTAssertTrue(app.staticTexts["pair.pairedTitle"].waitForExistence(timeout: 10), "D4 missing")
        snap(app, "D4-paired")
        app.buttons["pair.goToRoutes"].tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing after pairing")
    }

    /// A scan timeout resolves to the failure screen; Try again loops back through scanning.
    @MainActor
    func testPairingTimeoutShowsD5AndRetryLoops() {
        let app = launch(scenario: "pairingTimeout")

        XCTAssertTrue(app.staticTexts["pair.introTitle"].waitForExistence(timeout: 10))
        app.buttons["pair.start"].tap()

        let failed = app.staticTexts["pair.failedTitle"]
        XCTAssertTrue(failed.waitForExistence(timeout: 10), "D5 missing")
        XCTAssertEqual(failed.label, "Couldn't find your OBC")
        snap(app, "D5-timeout")

        app.buttons["pair.tryAgain"].tap()
        XCTAssertTrue(failed.waitForExistence(timeout: 10), "retry did not loop back to D5")
    }

    @MainActor
    func testPairingRejectedShowsD5RejectedCopy() {
        let app = launch(scenario: "pairingRejected")

        XCTAssertTrue(app.staticTexts["pair.introTitle"].waitForExistence(timeout: 10))
        app.buttons["pair.start"].tap()

        // The row appears first, from un-gated discovery. The passkey is gated and only fires on
        // the row tap, so a rejection surfaces after confirming, not before.
        let row = app.buttons["pair.deviceRow"]
        XCTAssertTrue(row.waitForExistence(timeout: 10), "D2 discovered row missing")
        row.tap()

        let failed = app.staticTexts["pair.failedTitle"]
        XCTAssertTrue(failed.waitForExistence(timeout: 10), "D5 missing")
        XCTAssertEqual(failed.label, "Pairing didn't finish")
    }

    /// Bluetooth off has its own screen, and the library never locks.
    @MainActor
    func testBluetoothOffShowsH8AndLibraryStaysReachable() {
        let app = launch(scenario: "bluetoothOff")

        XCTAssertTrue(app.staticTexts["pair.introTitle"].waitForExistence(timeout: 10))
        app.buttons["pair.start"].tap()

        let title = app.staticTexts["radio.title"]
        XCTAssertTrue(title.waitForExistence(timeout: 10), "H8 missing")
        XCTAssertEqual(title.label, "Bluetooth is off")
        XCTAssertTrue(app.buttons["radio.tryAgain"].exists)
        snap(app, "H8-bluetooth-off")

        app.buttons["radio.browseLibrary"].tap()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
    }

    @MainActor
    func testPermissionDeniedShowsH7State() {
        let app = launch(scenario: "permissionDenied")

        XCTAssertTrue(app.staticTexts["pair.introTitle"].waitForExistence(timeout: 10))
        app.buttons["pair.start"].tap()

        let title = app.staticTexts["radio.title"]
        XCTAssertTrue(title.waitForExistence(timeout: 10), "H7 state missing")
        XCTAssertEqual(title.label, "Allow Bluetooth access")
    }

    /// A bonded launch goes straight to the main screen: no pairing prompt, no error.
    @MainActor
    func testBondedLaunchLandsOnMain() {
        let app = launch(scenario: "happyPath")
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.staticTexts["pair.introTitle"].exists)
    }

    /// Bonded and out of range: the main screen with its banner, never an error screen.
    @MainActor
    func testOutOfRangeLandsOnMainWithDisconnectedBanner() {
        let app = launch(scenario: "outOfRange")
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.otherElements["disconnectedBanner"].firstMatch.exists
                      || app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'out of range'")).firstMatch.exists,
                      "S4 banner missing")
        snap(app, "S4-main-out-of-range")
    }

    /// Bonded but with the link down at launch: the connecting state resolves to the main screen
    /// within the grace window.
    @MainActor
    func testBondedColdLaunchResolvesToMain() {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", "happyPath", "-OBCConnection", "disconnected"]
        app.launch()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 15))
    }

    /// Bonded but the device never answers: the grace window expires onto the connect-failed
    /// screen, never a forever-spinner, and the secondary action still reaches the library.
    @MainActor
    func testDeviceUnreachableTimesOutToConnectFailedAndRoutesStayReachable() {
        let app = launch(scenario: "deviceUnreachable")

        XCTAssertTrue(app.staticTexts["launch.connectingTitle"].waitForExistence(timeout: 10), "A state missing")
        // The default connect grace must expire onto the timeout screen.
        let title = app.staticTexts["launch.connectFailedTitle"]
        XCTAssertTrue(title.waitForExistence(timeout: 15), "connect-failed screen missing")
        XCTAssertEqual(title.label, "Can't reach Trailhead")
        XCTAssertTrue(app.buttons["launch.tryAgain"].exists)
        snap(app, "A-timeout-connect-failed")

        // Try again re-enters the connecting state, and the still-silent device times out again.
        app.buttons["launch.tryAgain"].tap()
        XCTAssertTrue(app.staticTexts["launch.connectingTitle"].waitForExistence(timeout: 10), "retry must re-enter A")
        XCTAssertTrue(title.waitForExistence(timeout: 15), "second timeout missing")

        app.buttons["launch.goToRoutes"].tap()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "library must stay reachable")
    }
}
