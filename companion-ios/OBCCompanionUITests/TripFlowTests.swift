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

    /// The import landing's Start a trip row makes the import the first day of a new trip and
    /// opens the day editor in split mode: the stepper cuts the file into days, and Done lands on
    /// the trip page with those days.
    @MainActor
    func testImportStartsATrip() {
        let app = launch(fixtures: "trips", importSample: "gpx")

        let row = app.buttons["import.startTrip"]
        XCTAssertTrue(row.waitForExistence(timeout: 10), "import Start a trip row missing")
        XCTAssertTrue(app.buttons["import.addToTrip"].exists, "the one fixture trip must be offered")
        snap(app, "TR7-import-rows")
        row.tap()

        let stepper = app.steppers["dayEditor.days"]
        XCTAssertTrue(stepper.waitForExistence(timeout: 10), "Start a trip must open the day editor in split mode")
        XCTAssertTrue(app.navigationBars["Schwarzwald Tour · Tag 2"].exists, "the trip takes the route's name")
        snap(app, "DE-split-1")
        app.buttons["dayEditor.days-Increment"].tap()
        XCTAssertTrue(app.buttons["dayEditor.day.1"].firstMatch.waitForExistence(timeout: 5), "one tap must make two days")
        app.buttons["dayEditor.days-Increment"].tap()
        let day3 = app.buttons["dayEditor.day.2"].firstMatch
        XCTAssertTrue(day3.waitForExistence(timeout: 5), "two stepper taps must make three days")
        XCTAssertTrue(app.staticTexts["3 DAYS · ~1 H A DAY"].exists, "the header counts the days")
        snap(app, "DE-split-3")

        // A drag on the profile handle moves the day end; Even out balances again; Undo steps back.
        let handle = app.otherElements["Day 2 end"].firstMatch
        XCTAssertTrue(handle.exists, "the profile handle is missing")
        let balanced = day3.label
        let start = handle.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        start.press(forDuration: 0.2, thenDragTo: start.withOffset(CGVector(dx: 70, dy: 0)))
        XCTAssertNotEqual(day3.label, balanced, "the drag must move the day end")
        let dragged = day3.label
        snap(app, "DE-split-dragged")
        // Even out days is the last row: raise the sheet to its full height first.
        let grabber = app.buttons["Sheet Grabber"].firstMatch
        XCTAssertTrue(grabber.exists, "the sheet grabber is missing")
        grabber.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
            .press(forDuration: 0.1, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.08)))
        let evenOut = app.buttons["dayEditor.evenOut"]
        XCTAssertTrue(evenOut.waitForExistence(timeout: 5), "Even out days row missing")
        evenOut.tap()
        XCTAssertNotEqual(day3.label, dragged, "Even out must move the day end again")
        app.buttons["dayEditor.undo"].tap()
        XCTAssertEqual(day3.label, dragged, "Undo must step back to the dragged position")
        app.buttons["dayEditor.undo"].tap()
        XCTAssertEqual(day3.label, balanced, "a second Undo steps back the drag")
        app.buttons["dayEditor.done"].tap()

        XCTAssertTrue(
            app.descendants(matching: .any)["trip.day.2"].firstMatch.waitForExistence(timeout: 10),
            "Done must save the three days and land on the trip page")
        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'main.card.'"))
            .matching(NSPredicate(format: "label CONTAINS 'Schwarzwald'")).firstMatch.exists,
            "the imported route must not also be a route card")
    }

    // MARK: Day editor

    /// Edit days on the trip page opens the editor in edit mode. The fixture trip's two files
    /// do not join, so Day 1 ends at a transfer: its row says so. Add day end arms placement
    /// and a tap on the profile puts the new end there; its row's long press ends the day at a
    /// searched stop; the back chevron asks before it discards all of it.
    @MainActor
    func testEditDaysPlacementStopAndDiscard() {
        let app = launch()
        waitForMain(app)

        app.buttons["main.trip.driftless-weekender"].tap()
        let editDays = app.buttons["trip.editDays"]
        XCTAssertTrue(editDays.waitForExistence(timeout: 5), "Edit days missing on the trip page")
        editDays.tap()

        let day1 = app.buttons["dayEditor.day.0"].firstMatch
        XCTAssertTrue(day1.waitForExistence(timeout: 10), "the day editor is missing")
        XCTAssertTrue(app.buttons["dayEditor.evenOut"].exists, "edit mode has Even out in the header")
        XCTAssertFalse(app.buttons["dayEditor.days-Increment"].exists, "edit mode has no stepper")
        XCTAssertTrue(day1.label.hasSuffix("transfer"), "a day end at a transfer says so: \(day1.label)")
        snap(app, "DE-edit")

        // Add day end: the hint shows, a tap on the profile places the end inside Day 1.
        app.buttons["dayEditor.addDayEnd"].tap()
        XCTAssertTrue(app.staticTexts["dayEditor.placementHint"].waitForExistence(timeout: 5), "placement hint missing")
        snap(app, "DE-edit-placing")
        let plot = app.otherElements["profile.placement"].firstMatch
        XCTAssertTrue(plot.waitForExistence(timeout: 5), "the profile takes no placement")
        plot.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.6)).tap()
        let day3 = app.buttons["dayEditor.day.2"].firstMatch
        XCTAssertTrue(day3.waitForExistence(timeout: 5), "the tap must add a third day")
        XCTAssertFalse(app.staticTexts["dayEditor.placementHint"].exists, "placement ends with the tap")
        snap(app, "DE-edit-placed")

        // The new day's row: long press, End the day at a stop, search, pick.
        day1.press(forDuration: 1)
        let stops = app.buttons["dayEditor.day.stops"]
        XCTAssertTrue(stops.waitForExistence(timeout: 5), "the row menu did not open")
        stops.tap()
        let field = app.textFields["stops.search"].firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 10), "the stops sheet is missing")
        field.tap()
        field.typeText("Devil\n")
        let camp = app.buttons["stops.row"].firstMatch
        XCTAssertTrue(camp.waitForExistence(timeout: 10), "no stop found")
        XCTAssertTrue(camp.isEnabled, "the campground must be inside the new day")
        snap(app, "DE-edit-stops")
        camp.tap()
        XCTAssertTrue(
            app.buttons["dayEditor.day.0"].firstMatch.label.contains("Devil's Lake State Park Campgrounds"),
            "the day end takes the stop's name: \(app.buttons["dayEditor.day.0"].firstMatch.label)")
        snap(app, "DE-edit-at-stop")

        // Discard: the standard alert, then the trip page with its two days as saved.
        app.buttons["dayEditor.back"].tap()
        let alert = app.alerts["Discard changes?"]
        XCTAssertTrue(alert.waitForExistence(timeout: 5), "the discard alert is missing")
        snap(app, "DE-edit-discard")
        alert.buttons["Keep Editing"].tap()
        XCTAssertTrue(day3.waitForExistence(timeout: 5), "Keep Editing keeps the draft")
        app.buttons["dayEditor.back"].tap()
        XCTAssertTrue(alert.waitForExistence(timeout: 5))
        alert.buttons["Discard"].tap()
        XCTAssertTrue(app.buttons["trip.editDays"].waitForExistence(timeout: 5), "Discard must return to the trip page")
        XCTAssertFalse(app.descendants(matching: .any)["trip.day.2"].firstMatch.exists, "the discarded day end must not be saved")
    }
}
