import XCTest

/// The ride timeline driven through the real UI against the effort fixture: a ride with zones, a
/// ride with sensors but no limits, and a ride without sensors. The series and zone maths is
/// host-tested in OBCKit's `RideTimelineTests`.
final class RideTimelineUITests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func element(_ app: XCUIApplication, _ id: String) -> XCUIElement {
        app.descendants(matching: .any)[id].firstMatch
    }

    @MainActor
    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        // The fixture rides are already synced, so the Tracked list has them at launch.
        app.launchArguments += [
            "-OBCScenario", "syncUpToDate", "-OBCFixtures", "effort",
            "-OBCHideMockHUD", "-OBCDisableAnimations",
            "-AppleLanguages", "(en)", "-AppleLocale", "en_US",
        ]
        app.launch()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
        app.buttons["Rides"].tap()
        return app
    }

    @MainActor
    private func openRide(_ app: XCUIApplication, _ id: String) {
        let card = app.buttons["main.card.\(id)"]
        XCTAssertTrue(card.waitForExistence(timeout: 10), "\(id) missing")
        card.tap()
        XCTAssertTrue(element(app, "detail.timeline").waitForExistence(timeout: 5), "timeline missing")
    }

    @MainActor
    private func expandStatistics(_ app: XCUIApplication) {
        let statistics = app.buttons["detail.statistics"]
        for _ in 0..<4 where !statistics.isHittable { app.swipeUp() }
        XCTAssertTrue(statistics.isHittable, "statistics disclosure missing")
        statistics.tap()
    }

    @MainActor
    func testADragMovesTheCursorAndALabelOpensTheChannel() {
        let app = launch()
        openRide(app, "ride-furka-pass")
        for channel in ["elevation", "speed", "heartRate", "power", "cadence"] {
            XCTAssertTrue(app.buttons["timeline.\(channel)"].exists, "\(channel) strip missing")
        }
        XCTAssertFalse(element(app, "zones.heartRate").exists, "zones start collapsed")
        XCTAssertFalse(element(app, "ledger.Avg speed").exists, "secondary statistics start collapsed")

        let position = element(app, "timeline.position")
        XCTAssertEqual(position.value as? String, "Averages")
        let card = element(app, "detail.timeline")
        card.coordinate(withNormalizedOffset: CGVector(dx: 0.35, dy: 0.6))
            .press(forDuration: 0.05, thenDragTo: card.coordinate(withNormalizedOffset: CGVector(dx: 0.6, dy: 0.62)))
        XCTAssertTrue((position.value as? String)?.hasPrefix("Kilometre") == true, "the drag did not scrub")

        app.buttons["timeline.heartRate"].tap()
        XCTAssertTrue(element(app, "channel.sheet").waitForExistence(timeout: 5), "detail sheet missing")
        XCTAssertTrue(element(app, "channel.readout").label.hasPrefix("KM"), "the sheet lost the cursor")
        let sheet = element(app, "channel.sheet")
        sheet.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.02))
            .press(forDuration: 0.05, thenDragTo: sheet.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.95)))
        XCTAssertTrue(sheet.waitForNonExistence(timeout: 5), "channel sheet did not close")
        expandStatistics(app)
        XCTAssertTrue(element(app, "zones.heartRate").exists, "heart rate zones missing")
        XCTAssertTrue(element(app, "zones.power").exists, "power zones missing")
        XCTAssertFalse(element(app, "ledger.Distance").exists, "summary totals must not repeat")
        app.buttons["detail.statistics"].tap()
        XCTAssertFalse(element(app, "zones.heartRate").exists, "zones collapse with secondary statistics")
    }

    @MainActor
    func testARideWithoutLimitsOrSensorsSaysWhatItLacks() {
        let app = launch()
        openRide(app, "ride-grimsel-strap")
        XCTAssertTrue(app.buttons["timeline.heartRate"].exists)
        expandStatistics(app)
        XCTAssertEqual(
            element(app, "zones.none").label,
            "No zones for this ride: max heart rate was not set on the device."
        )
        XCTAssertFalse(element(app, "zones.heartRate").exists)

        app.navigationBars.buttons.firstMatch.tap()
        openRide(app, "ride-gletsch-descent")
        XCTAssertTrue(app.buttons["timeline.elevation"].exists)
        XCTAssertTrue(app.buttons["timeline.speed"].exists)
        XCTAssertFalse(app.buttons["timeline.heartRate"].exists, "no strip without samples")
        expandStatistics(app)
        XCTAssertFalse(element(app, "zones.none").exists, "no zones line without heart rate or power")
    }
}
