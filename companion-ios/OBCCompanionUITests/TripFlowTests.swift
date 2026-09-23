import XCTest

/// The create and file flows end to end against the trips fixture: multi-select grouping, the
/// detail overflow, and the import row's new-trip option. Model logic is host-tested in
/// `TripFlowModelTests`; this proves the wiring.
final class TripFlowTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch(fixtures: String = "trips", importSample: String? = nil) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", "happyPath", "-OBCFixtures", fixtures]
        if let importSample { app.launchArguments += ["-OBCImportSample", importSample] }
        // Pin the locale so English strings assert cleanly.
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
    private func waitForMain(_ app: XCUIApplication) {
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
    }

    // MARK: Multi-select grouping

    /// Select two loose routes and group them: a trip card appears in their place, and the grouped
    /// routes leave the top level.
    @MainActor
    func testMultiSelectGroupingEndToEnd() {
        let app = launch()
        waitForMain(app)

        app.buttons["main.select"].tap()
        // The group action is disabled until two routes are picked.
        let group = app.buttons["main.groupIntoTrip"]
        XCTAssertTrue(group.waitForExistence(timeout: 5), "group action bar missing")
        XCTAssertFalse(group.isEnabled, "group must need at least two routes")

        app.buttons["main.card.kettle-moraine-loop"].tap()
        app.buttons["main.card.sugar-river-trail"].tap()
        XCTAssertTrue(group.isEnabled, "two routes selected should enable grouping")
        snap(app, "TR7-multiselect")
        group.tap()

        let alert = app.alerts["Name the trip"]
        XCTAssertTrue(alert.waitForExistence(timeout: 5), "name prompt missing")
        let field = alert.textFields.firstMatch
        field.tap()
        field.clearText()
        field.typeText("Northwoods Weekend")
        alert.buttons["Create"].tap()

        // The trip card appears, and the two grouped routes left the list.
        XCTAssertTrue(app.staticTexts["Northwoods Weekend"].waitForExistence(timeout: 5), "new trip card missing")
        XCTAssertFalse(app.buttons["main.card.kettle-moraine-loop"].exists, "grouped route still loose")
        XCTAssertFalse(app.buttons["main.card.sugar-river-trail"].exists, "grouped route still loose")
        snap(app, "TR7-grouped")
    }

    // MARK: Detail overflow → Add to trip

    /// A route's detail overflow adds it to an existing trip as a new last day: the trip page
    /// replaces the route page, and the route leaves the list.
    @MainActor
    func testDetailOverflowAddsTheRouteToATrip() {
        let app = launch()
        waitForMain(app)

        app.buttons["main.card.kettle-moraine-loop"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5))
        app.buttons["detail.overflow"].tap()
        let addToTrip = app.buttons["detail.addToTrip"]
        XCTAssertTrue(addToTrip.waitForExistence(timeout: 5), "overflow Add to trip… missing")
        addToTrip.tap()
        app.buttons["tripPicker.trip.driftless-weekender"].tap()

        XCTAssertTrue(
            app.descendants(matching: .any)["trip.day.2"].firstMatch.waitForExistence(timeout: 5),
            "the route is not the trip's third day")
        snap(app, "TR7-detail-added")

        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["main.card.kettle-moraine-loop"].waitForExistence(timeout: 3),
                       "a route added to a trip must leave the list")
    }

    // MARK: Import row → New trip

    /// The import-save screen's optional Add-to-trip row makes the new import the first day of a
    /// fresh trip on save.
    @MainActor
    func testImportWithNewTrip() {
        let app = launch(fixtures: "trips", importSample: "gpx")

        // The landing is up; find the opt-in Add-to-trip row.
        let row = app.buttons["import.addToTrip"]
        XCTAssertTrue(row.waitForExistence(timeout: 10), "import Add to trip row missing")
        for _ in 0..<4 where !row.isHittable { app.swipeUp(velocity: .fast) }
        row.tap()

        let newTripRow = app.buttons["tripPicker.newTrip"]
        XCTAssertTrue(newTripRow.waitForExistence(timeout: 5), "picker missing")
        newTripRow.tap()
        let nameField = app.textFields["tripPicker.newName"]
        XCTAssertTrue(nameField.waitForExistence(timeout: 5), "new-trip name field missing")
        nameField.tap()
        nameField.clearText()
        nameField.typeText("Schwarzwald Trip")
        app.buttons["tripPicker.create"].tap()
        snap(app, "TR7-import-newtrip")

        // Save the import; it lands as the new trip's first day.
        let save = app.buttons["detail.saveToPlanned"]
        for _ in 0..<4 where !save.isHittable { app.swipeUp(velocity: .fast) }
        save.tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5))
        let tripCard = app.staticTexts["Schwarzwald Trip"]
        XCTAssertTrue(tripCard.waitForExistence(timeout: 5), "new trip card missing after import")
        // The imported route is part of the trip, not a route card.
        XCTAssertFalse(app.staticTexts["Schwarzwald Tour · Tag 2"].exists, "imported route leaked to top level")
        tripCard.tap()
        XCTAssertTrue(
            app.descendants(matching: .any)["trip.day.0"].firstMatch.waitForExistence(timeout: 5),
            "imported route is not the first day of the new trip")
    }
}
