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
        app.launchArguments += ["-OBCScenario", scenario, "-OBCHideMockHUD", "-OBCDisableAnimations"]
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

    @MainActor
    private func enterPairing(_ app: XCUIApplication, capture: Bool = false) {
        XCTAssertTrue(app.staticTexts["onboarding.welcomeTitle"].waitForExistence(timeout: 10))
        if capture { snap(app, "onboarding-P01-welcome") }
        app.buttons["onboarding.getStarted"].tap()
        XCTAssertTrue(app.staticTexts["onboarding.switchOnTitle"].waitForExistence(timeout: 10))
        if capture { snap(app, "onboarding-P02-switch-on") }
        app.buttons["pair.start"].tap()
        XCTAssertTrue(app.staticTexts["onboarding.bluetoothTitle"].waitForExistence(timeout: 10))
        if capture { snap(app, "onboarding-P03-bluetooth") }
        app.buttons["onboarding.allowBluetooth"].tap()
    }

    /// The intro, the scan with the row sliding in, the pairing beat, then the main screen.
    @MainActor
    func testFirstRunPairingHappyPath() {
        let app = launch(scenario: "onboarding")

        enterPairing(app, capture: true)

        XCTAssertTrue(app.staticTexts["pair.pairedTitle"].waitForExistence(timeout: 10), "D4 missing")
        snap(app, "onboarding-P06-paired")
        app.buttons["pairing.keepName"].tap()
        XCTAssertTrue(app.staticTexts["onboarding.sensorsTitle"].waitForExistence(timeout: 10))
        snap(app, "onboarding-P08-sensors")
        app.buttons["onboarding.sensorsContinue"].tap()
        XCTAssertTrue(app.staticTexts["onboarding.routeTitle"].waitForExistence(timeout: 10))
        snap(app, "onboarding-P10-route")
        app.buttons["onboarding.routeSkip"].tap()
        XCTAssertTrue(app.staticTexts["onboarding.rideTitle"].waitForExistence(timeout: 10))
        snap(app, "onboarding-P11-ride")
        app.buttons["onboarding.finish"].tap()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing after setup")
        XCTAssertTrue(app.descendants(matching: .any)["onboarding.readyNote"].firstMatch.exists)
        snap(app, "onboarding-P12-library")
    }

    /// A scan timeout resolves to the failure screen; Try again loops back through scanning.
    @MainActor
    func testPairingTimeoutShowsD5AndRetryLoops() {
        let app = launch(scenario: "pairingTimeout")

        enterPairing(app)

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

        enterPairing(app)

        let failed = app.staticTexts["pair.failedTitle"]
        XCTAssertTrue(failed.waitForExistence(timeout: 10), "D5 missing")
        XCTAssertEqual(failed.label, "Pairing didn't finish")
    }

    /// Bluetooth off has its own screen, and the library never locks.
    @MainActor
    func testBluetoothOffShowsH8AndLibraryStaysReachable() {
        let app = launch(scenario: "bluetoothOff")

        enterPairing(app)

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

        enterPairing(app)

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

    /// A saved pairing opens the Library while the link connects in the background.
    @MainActor
    func testBondedColdLaunchResolvesToMain() {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", "happyPath", "-OBCConnection", "disconnected"]
        app.launch()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 15))
    }

    /// An absent device leaves the Library available, with connection status in its header.
    @MainActor
    func testDeviceUnreachableOpensLibraryWhileReconnecting() {
        let app = launch(scenario: "deviceUnreachable")
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "library must stay reachable")
        let header = app.descendants(matching: .any)["topbar.device"].firstMatch
        let reconnecting = expectation(
            for: NSPredicate(format: "label CONTAINS[c] 'connecting'"), evaluatedWith: header)
        wait(for: [reconnecting], timeout: 10)
        XCTAssertFalse(app.buttons["topbar.sync"].isEnabled)
        XCTAssertTrue(app.buttons["topbar.settings"].isEnabled)
        XCTAssertFalse(app.staticTexts["launch.connectingTitle"].exists)
        XCTAssertFalse(app.staticTexts["launch.connectFailedTitle"].exists)
        app.segmentedControls.buttons["Rides"].tap()
        XCTAssertTrue(app.staticTexts["No rides yet"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.descendants(matching: .any)["main.readError"].firstMatch.exists)
        snap(app, "D5-library-offline")
    }
}
