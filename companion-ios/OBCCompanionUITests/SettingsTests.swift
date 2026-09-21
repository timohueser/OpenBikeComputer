import XCTest

/// Settings reached from the top-bar gear, the device rename showing across the app, and the
/// forget returning to the unpaired prompt. The model logic is host-tested in `SettingsModelTests`;
/// this proves the wiring from gear to screen to alerts to app state.
final class SettingsTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch(scenario: String = "happyPath") -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", scenario]
        app.launchArguments += ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        return app
    }

    @MainActor
    private func snap(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    /// The screen id sits on a `ScrollView`, so query descendants.
    @MainActor
    private func openSettings(_ app: XCUIApplication) {
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
        app.buttons["topbar.settings"].tap()
        let screen = app.descendants(matching: .any)["settings.screen"]
        XCTAssertTrue(screen.waitForExistence(timeout: 5), "settings screen missing")
    }

    /// Every group renders. The screen scrolls past a fold, so reveal the lower groups before
    /// asserting them.
    @MainActor
    func testSettingsShowsTheFourDesignGroups() {
        let app = launch()
        openSettings(app)

        XCTAssertTrue(app.staticTexts["Trailhead"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Connected · 82%"].waitForExistence(timeout: 5),
                      "device status line missing")
        XCTAssertTrue(app.staticTexts["Rename device"].exists)
        XCTAssertTrue(app.staticTexts["Forget device"].exists)
        snap(app, "G-settings")

        // Each lower group scrolls off the top as the next comes up, so assert each as it appears.
        reveal(app, "Update firmware")
        reveal(app, "Strava sync")
        reveal(app, "OpenBikeComputer on GitHub")
        reveal(app, "No account. No subscription. No cloud.")
    }

    /// Swipe up until the labelled row enters the tree, then assert it.
    @MainActor
    private func reveal(_ app: XCUIApplication, _ label: String) {
        let element = app.staticTexts[label]
        for _ in 0..<6 where !element.exists { app.swipeUp(velocity: .fast) }
        XCTAssertTrue(element.exists, "\(label) row missing")
    }

    /// Rename through the text-field alert; the new name shows in Settings and on the main top bar.
    @MainActor
    func testRenameDeviceShowsAcrossTheApp() {
        let app = launch()
        openSettings(app)

        app.staticTexts["Rename device"].tap()
        let alert = app.alerts["Rename device"]
        XCTAssertTrue(alert.waitForExistence(timeout: 5), "H3 alert missing")
        snap(app, "H3-rename-device")

        let field = alert.textFields.firstMatch
        field.tap()
        field.clearText()
        field.typeText("Summit")
        alert.buttons["Save"].tap()

        XCTAssertTrue(app.staticTexts["Summit"].waitForExistence(timeout: 5),
                      "settings kept the old name")

        app.navigationBars.buttons.firstMatch.tap()  // back to the main screen
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Summit"].waitForExistence(timeout: 5),
                      "top bar kept the old name")
    }

    /// Forget confirms with the reassurance copy, then lands on the pairing prompt.
    @MainActor
    func testForgetDeviceConfirmsThenReturnsToPairing() {
        let app = launch()
        openSettings(app)

        app.staticTexts["Forget device"].tap()
        // Scoped to the sheet: the row shares the "Forget device" label.
        let confirm = app.sheets.buttons["Forget device"]
        XCTAssertTrue(confirm.waitForExistence(timeout: 5), "H2 confirm missing")
        XCTAssertTrue(
            app.sheets.staticTexts["You'll pair again to use it. Your routes and rides stay on this phone."]
                .exists,
            "H2 reassurance copy missing")
        snap(app, "H2-forget-confirm")
        confirm.tap()

        XCTAssertTrue(app.staticTexts["pair.introTitle"].waitForExistence(timeout: 10),
                      "forget should land on the D1 pairing prompt")
        snap(app, "H2-after-forget-D1")
    }

    /// Cancelling the forget keeps the bond: still on Settings, still bonded.
    @MainActor
    func testForgetCancelKeepsEverything() {
        let app = launch()
        openSettings(app)

        app.staticTexts["Forget device"].tap()
        XCTAssertTrue(app.sheets.buttons["Forget device"].waitForExistence(timeout: 5))
        // Dismiss without confirming: the dialog's Cancel is not a queryable button on this iOS
        // version, and tapping the scrim is the same user gesture.
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.15)).tap()

        XCTAssertTrue(app.descendants(matching: .any)["settings.screen"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Trailhead"].exists)
    }
}
