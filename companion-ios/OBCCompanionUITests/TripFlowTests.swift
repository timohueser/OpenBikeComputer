import XCTest

/// TR7 acceptance on the simulator: the create & file flows end to end against
/// the `trips` fixture — multi-select grouping, the route card context menu, the
/// detail overflow (add / move / remove), and the import row's "New trip…".
/// Model logic is host-tested in `TripFlowModelTests`; this proves the wiring.
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
        // Pin the locale so en-US strings assert cleanly.
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

    /// Select two loose routes → Group into trip… → name → a trip card appears
    /// in their place and the grouped routes leave the top level.
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

        // The trip card appears; the two grouped routes are no longer loose.
        XCTAssertTrue(app.staticTexts["Northwoods Weekend"].waitForExistence(timeout: 5), "new trip card missing")
        XCTAssertFalse(app.buttons["main.card.kettle-moraine-loop"].exists, "grouped route still loose")
        XCTAssertFalse(app.buttons["main.card.sugar-river-trail"].exists, "grouped route still loose")
        snap(app, "TR7-grouped")
    }

    // MARK: Card context menu → New trip

    /// Long-press a loose route → Add to trip… → New trip… → the route files into
    /// a fresh trip and leaves the top level.
    @MainActor
    func testAddToTripViaCardContextMenuNewTrip() {
        let app = launch()
        waitForMain(app)

        app.buttons["main.card.blue-mounds-backroads"].press(forDuration: 1.1)
        let addItem = app.buttons.matching(
            NSPredicate(format: "label CONTAINS 'Add to trip'")).firstMatch
        XCTAssertTrue(addItem.waitForExistence(timeout: 5), "context menu Add to trip… missing")
        addItem.tap()

        let newTrip = app.buttons["tripPicker.newTrip"]
        XCTAssertTrue(newTrip.waitForExistence(timeout: 5), "picker missing")
        newTrip.tap()
        let create = app.buttons["tripPicker.create"]
        XCTAssertTrue(create.waitForExistence(timeout: 5), "new-trip create missing")
        create.tap()  // default name "New trip"

        // The route filed into the new trip; a "New trip" card is now present.
        XCTAssertTrue(app.staticTexts["New trip"].waitForExistence(timeout: 5), "new trip card missing")
        XCTAssertFalse(app.buttons["main.card.blue-mounds-backroads"].exists, "filed route still loose")
    }

    // MARK: Detail overflow → add, then move + remove

    /// A loose route's detail overflow files it into an existing trip; a filed
    /// route's detail overflow offers Move to trip… and Remove from trip.
    @MainActor
    func testDetailOverflowAddMoveRemove() {
        let app = launch()
        waitForMain(app)

        // Add a loose route to the Driftless trip from its detail overflow.
        app.buttons["main.card.kettle-moraine-loop"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5))
        app.buttons["detail.overflow"].tap()
        let addToTrip = app.buttons["detail.addToTrip"]
        XCTAssertTrue(addToTrip.waitForExistence(timeout: 5), "overflow Add to trip… missing")
        addToTrip.tap()
        app.buttons["tripPicker.trip.driftless-weekender"].tap()
        snap(app, "TR7-detail-added")

        // Back on the list, the route is filed (no longer a loose card).
        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["main.card.kettle-moraine-loop"].waitForExistence(timeout: 3),
                       "route added to a trip must leave the top level")

        // Open the trip, open a stage, and Remove from trip via the overflow.
        app.buttons["main.trip.driftless-weekender"].tap()
        let stage = app.buttons["trip.stage.kettle-moraine-loop"]
        XCTAssertTrue(stage.waitForExistence(timeout: 5), "added stage missing from the trip")
        stage.tap()
        XCTAssertTrue(app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5))
        app.buttons["detail.overflow"].tap()
        XCTAssertTrue(app.buttons["detail.moveToTrip"].waitForExistence(timeout: 5),
                      "a filed route must offer Move to trip…")
        app.buttons["detail.removeFromTrip"].tap()

        // The route returns to the top level: pop detail → trip page → main list.
        app.navigationBars.buttons.element(boundBy: 0).tap()  // detail → trip page
        app.navigationBars.buttons.element(boundBy: 0).tap()  // trip page → main
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["main.card.kettle-moraine-loop"].waitForExistence(timeout: 5),
                      "removed route did not return to the top level")
    }

    // MARK: Import row → New trip

    /// The import-save screen's optional "Add to trip" row (opt-in) files the new
    /// import into a fresh trip on save.
    @MainActor
    func testImportWithNewTrip() {
        let app = launch(fixtures: "trips", importSample: "gpx")

        // The E1 landing is up; find the opt-in Add to trip row.
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

        // Save the import; it lands filed in the new trip.
        let save = app.buttons["detail.saveToPlanned"]
        for _ in 0..<4 where !save.isHittable { app.swipeUp(velocity: .fast) }
        save.tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5))
        let tripCard = app.staticTexts["Schwarzwald Trip"]
        XCTAssertTrue(tripCard.waitForExistence(timeout: 5), "new trip card missing after import")
        // The imported route is filed, not a loose top-level card.
        XCTAssertFalse(app.staticTexts["Schwarzwald Tour · Tag 2"].exists, "imported route leaked to top level")
        tripCard.tap()
        XCTAssertTrue(app.staticTexts["Schwarzwald Tour · Tag 2"].waitForExistence(timeout: 5),
                      "imported route is not a stage of the new trip")
    }
}
