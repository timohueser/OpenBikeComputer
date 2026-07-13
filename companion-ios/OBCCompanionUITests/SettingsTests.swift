import XCTest

/// B8 acceptance on the simulator: Settings (G) reached from the top-bar gear,
/// the H3 device rename showing across the app, and the H2 forget returning to
/// the unpaired D1 prompt. The model logic is host-tested in
/// `SettingsModelTests`; this proves the wiring gear → G → alerts → app state.
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

    /// Gear → G. The screen id sits on a ScrollView, so query descendants
    /// (same gotcha as `detail.screen`).
    @MainActor
    private func openSettings(_ app: XCUIApplication) {
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
        app.buttons["topbar.settings"].tap()
        let screen = app.descendants(matching: .any)["settings.screen"]
        XCTAssertTrue(screen.waitForExistence(timeout: 5), "settings screen missing")
    }

    /// G: every group renders — device identity, the coming-soon rows, About.
    @MainActor
    func testSettingsShowsTheFourDesignGroups() {
        let app = launch()
        openSettings(app)

        XCTAssertTrue(app.staticTexts["Trailhead"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Connected · 82%"].waitForExistence(timeout: 5),
                      "device status line missing")
        XCTAssertTrue(app.staticTexts["Rename device"].exists)
        XCTAssertTrue(app.staticTexts["Forget device"].exists)
        XCTAssertTrue(app.staticTexts["Update over the air"].exists)
        XCTAssertTrue(app.staticTexts["v0.4.2 · latest"].exists, "firmware line missing")
        XCTAssertTrue(app.staticTexts["Strava sync"].exists)
        XCTAssertTrue(app.staticTexts["Komoot sync"].exists)
        XCTAssertTrue(app.staticTexts["Auto-sync on import"].exists)
        XCTAssertTrue(app.staticTexts["OpenBikeComputer on GitHub"].exists)
        XCTAssertTrue(app.staticTexts["No account. No subscription. No cloud."].exists,
                      "the no-cloud footer is the epic's promise — keep it")
        snap(app, "G-settings")
    }

    /// H3: rename via the text-field alert; the new name shows in Settings and
    /// on the main top bar.
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

    /// H2: forget confirms with the reassurance copy, then lands on D1.
    @MainActor
    func testForgetDeviceConfirmsThenReturnsToPairing() {
        let app = launch()
        openSettings(app)

        app.staticTexts["Forget device"].tap()
        // Scoped to the sheet — the row shares the "Forget device" label.
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

    /// H2 cancel keeps the bond — still on Settings, still bonded.
    @MainActor
    func testForgetCancelKeepsEverything() {
        let app = launch()
        openSettings(app)

        app.staticTexts["Forget device"].tap()
        XCTAssertTrue(app.sheets.buttons["Forget device"].waitForExistence(timeout: 5))
        // Dismiss without confirming — the dialog's Cancel isn't a queryable
        // button on iOS 26, and tapping the scrim is the same user gesture.
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.15)).tap()

        XCTAssertTrue(app.descendants(matching: .any)["settings.screen"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Trailhead"].exists)
    }
}
