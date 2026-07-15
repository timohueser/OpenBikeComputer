import XCTest

/// S7 acceptance on the simulator (epic #638): the four retention surfaces —
/// the Settings default picker, the upload sheet's Auto-delete row, the
/// route-detail control + device-expiry line, and the library card countdown —
/// plus the old-firmware gate that hides every device-truth surface. Model logic
/// is host-tested in `RetentionUIModelTests` / `RouteExpiryFormatTests`; this
/// proves the wiring end to end against `OBCMock`.
final class RetentionUITests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch(oldFirmware: Bool = false) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", "happyPath"]
        if oldFirmware { app.launchArguments += ["-OBCOldFirmware"] }
        // OBCFormat localizes ("62,4 km" on a German sim) — pin en-US for the
        // design strings (retention labels, "Expires in 2 days").
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

    @MainActor
    private func openSettings(_ app: XCUIApplication) {
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
        app.buttons["topbar.settings"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["settings.screen"].waitForExistence(timeout: 5),
                      "settings screen missing")
    }

    // MARK: 1 — Settings default picker

    /// The Auto-delete default row shows "After 2 weeks" and the picker changes it.
    @MainActor
    func testSettingsDefaultRetentionPicker() {
        let app = launch()
        openSettings(app)

        XCTAssertTrue(app.staticTexts["Auto-delete new routes"].waitForExistence(timeout: 5),
                      "the default-retention row is missing")
        let value = app.staticTexts["settings.autoDelete.value"]
        XCTAssertTrue(value.waitForExistence(timeout: 5))
        XCTAssertEqual(value.label, "After 2 weeks", "the documented default must seed the row")
        snap(app, "settings-auto-delete")

        // Open the pull-down and pick a different level.
        app.buttons["settings.autoDelete"].tap()
        let option = app.buttons["After 1 month"]
        XCTAssertTrue(option.waitForExistence(timeout: 5), "picker options missing")
        option.tap()
        XCTAssertEqual(
            app.staticTexts["settings.autoDelete.value"].label, "After 1 month",
            "picking a level must update the row")
    }

    // MARK: 2 — Upload sheet Auto-delete row (seeded from the default)

    /// Opening the upload sheet for a not-on-device route shows the Auto-delete row
    /// seeded from the default, before the transfer starts.
    @MainActor
    func testUploadSheetShowsAutoDeleteSeededFromDefault() {
        let app = launch()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        app.buttons["main.card.blue-mounds-backroads"].tap()
        app.buttons["detail.upload"].tap()

        XCTAssertTrue(app.buttons["upload.begin"].waitForExistence(timeout: 5),
                      "the pre-transfer confirm is missing")
        let value = app.staticTexts["upload.autoDelete.value"]
        XCTAssertTrue(value.waitForExistence(timeout: 5), "the upload Auto-delete row is missing")
        XCTAssertEqual(value.label, "After 2 weeks", "the row must seed from the default")
        snap(app, "upload-auto-delete")
    }

    // MARK: 3 — Route detail control + device expiry line

    /// The Kettle Moraine fixture is on the device with a near-term expiry (one
    /// week, last used 5 d ago → ~2 d): its detail shows the Auto-delete row with
    /// the device's "Expires …" line, and the row edits.
    @MainActor
    func testRouteDetailShowsAndEditsRetention() {
        let app = launch()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        // The detail model snapshots the on-device state at push — wait for the
        // reconcile to land (the card's own countdown badge is the signal) so the
        // route is proven on the device before opening it.
        XCTAssertTrue(app.staticTexts["routeCard.expiry"].firstMatch.waitForExistence(timeout: 15),
                      "reconcile did not land the on-device expiry")
        app.buttons["main.card.kettle-moraine-loop"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["detail.screen"].waitForExistence(timeout: 5))

        let row = app.buttons["detail.autoDelete"]
        for _ in 0..<5 where !row.isHittable { app.swipeUp(velocity: .fast) }
        XCTAssertTrue(row.waitForExistence(timeout: 5), "the detail Auto-delete row is missing")
        let expiry = app.staticTexts["detail.autoDelete.expiry"]
        XCTAssertTrue(expiry.waitForExistence(timeout: 5), "the device-expiry line is missing")
        XCTAssertTrue(expiry.label.hasPrefix("Expires"), "expiry line should read 'Expires …'")
        snap(app, "detail-auto-delete")

        // Edit it — the shown value updates in place (no pending chrome).
        row.tap()
        let option = app.buttons["After 2 months"]
        XCTAssertTrue(option.waitForExistence(timeout: 5), "detail picker options missing")
        option.tap()
        XCTAssertEqual(
            app.staticTexts["detail.autoDelete.value"].label, "After 2 months",
            "editing must update the shown value")
    }

    // MARK: 4 — Library card countdown badge

    /// The near-expiry route (≤ 3 days) carries the "Expires in N days" footnote on
    /// its library card.
    @MainActor
    func testLibraryCardShowsCountdownBadge() {
        let app = launch()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        let badge = app.staticTexts["routeCard.expiry"].firstMatch
        XCTAssertTrue(badge.waitForExistence(timeout: 10), "the near-expiry card badge is missing")
        XCTAssertTrue(badge.label.hasPrefix("Expires"), "badge should read 'Expires …'")
        snap(app, "library-expiry-badge")
    }

    // MARK: Old-firmware gate — no device-truth surfaces

    /// A device predating auto-expiry: no card badge, no detail row, and the
    /// upload sheet skips the confirm/row (it starts the transfer straight away).
    @MainActor
    func testOldFirmwareHidesEveryDeviceSurface() {
        let app = launch(oldFirmware: true)
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))

        // No countdown badge on any card.
        XCTAssertFalse(app.staticTexts["routeCard.expiry"].firstMatch.exists,
                       "old firmware must not badge expiry")

        // On-device route detail: no Auto-delete row.
        app.buttons["main.card.kettle-moraine-loop"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["detail.screen"].waitForExistence(timeout: 5))
        for _ in 0..<3 { app.swipeUp(velocity: .fast) }
        XCTAssertFalse(app.buttons["detail.autoDelete"].exists,
                       "old firmware must not show the detail retention row")

        // The upload sheet skips the confirm (no capability, nothing to set):
        // straight to the transfer, no Auto-delete row.
        app.navigationBars.buttons.firstMatch.tap()  // back to the list
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5))
        app.buttons["main.card.blue-mounds-backroads"].tap()
        app.buttons["detail.upload"].tap()
        XCTAssertTrue(app.staticTexts["Uploading to Trailhead"].waitForExistence(timeout: 5),
                      "old-firmware upload must start immediately")
        XCTAssertFalse(app.buttons["upload.begin"].exists, "no pre-transfer confirm on old firmware")
        XCTAssertFalse(app.staticTexts["upload.autoDelete.value"].exists,
                       "no Auto-delete row on old firmware")
    }
}
