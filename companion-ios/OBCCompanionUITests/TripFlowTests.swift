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

        let field = app.textFields["rename.field"]
        XCTAssertTrue(field.waitForExistence(timeout: 5), "name prompt missing")
        // Tap past the text's right end: clearing only deletes backwards from the caret.
        field.coordinate(withNormalizedOffset: CGVector(dx: 0.98, dy: 0.5)).tap()
        field.clearText()
        field.typeText("Northwoods Weekend")
        app.buttons["rename.save"].tap()

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
        XCTAssertTrue(
            app.staticTexts["dayEditor.summary"].label.hasPrefix("3 days"),
            "the header counts the days")
        snap(app, "DE-split-3")

        // A drag on the profile handle moves the day end; Undo steps back.
        let handle = profileHandle(app, "Day 2 end")
        XCTAssertTrue(handle.exists, "the profile handle is missing")
        let balanced = day3.label
        let start = handle.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        start.press(forDuration: 0.2, thenDragTo: start.withOffset(CGVector(dx: 70, dy: 0)))
        XCTAssertNotEqual(day3.label, balanced, "the drag must move the day end")
        snap(app, "DE-split-dragged")
        app.buttons["dayEditor.undo"].tap()
        XCTAssertEqual(day3.label, balanced, "Undo steps back the drag")
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

    /// Two files that arrive together become one trip through Make trip. A trip is a Planned row
    /// of its own: with no loose route left, the list still shows the trip card, not the empty
    /// state.
    @MainActor
    func testATripFromSeveralFilesStaysListed() {
        let app = launch(fixtures: "empty", importSample: "trip")

        let makeTrip = app.buttons["join.makeTrip"]
        XCTAssertTrue(makeTrip.waitForExistence(timeout: 10), "the Make a trip sheet is missing")
        makeTrip.tap()
        XCTAssertTrue(
            app.descendants(matching: .any)["trip.day.1"].firstMatch.waitForExistence(timeout: 10),
            "Make trip must open the new two-day trip's page")
        app.navigationBars.buttons.element(boundBy: 0).tap()

        waitForMain(app)
        let cards = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'main.trip.'"))
        XCTAssertTrue(cards.firstMatch.waitForExistence(timeout: 5), "the trip card is missing")
        XCTAssertEqual(cards.count, 1, "one trip must show one card")
        XCTAssertFalse(app.staticTexts["No planned routes yet"].exists, "a trip is not an empty list")
        snap(app, "trip-only-list")
    }

    // MARK: Day editor

    /// Edit days on the trip page opens the editor in edit mode. The fixture trip's two files
    /// do not join, so Day 1 ends at a transfer: its row says so. Day 1's menu splits it; a drag
    /// on the profile moves the new end; zoomed in, the map shows the stops, and a stop's
    /// callout ends the day there; the back chevron asks before it discards all of it.
    @MainActor
    func testEditDaysSplitDragStopAndDiscard() {
        let app = launch()
        waitForMain(app)

        app.buttons["main.trip.driftless-weekender"].tap()
        let editDays = app.buttons["trip.editDays"]
        XCTAssertTrue(editDays.waitForExistence(timeout: 5), "Edit days missing on the trip page")
        editDays.tap()

        let day1 = app.buttons["dayEditor.day.0"].firstMatch
        XCTAssertTrue(day1.waitForExistence(timeout: 10), "the day editor is missing")
        XCTAssertFalse(app.buttons["dayEditor.days-Increment"].exists, "edit mode has no stepper")
        XCTAssertTrue(day1.label.hasSuffix("transfer"), "a day end at a transfer says so: \(day1.label)")
        snap(app, "DE-edit")

        // Day 1's menu: a transfer end neither moves to a stop nor joins; Split this day cuts it.
        app.buttons["dayEditor.day.0.menu"].tap()
        XCTAssertFalse(app.buttons["dayEditor.day.join"].exists, "a transfer end cannot join")
        snap(app, "DE-edit-menu")
        app.buttons["dayEditor.day.split"].tap()
        let day3 = app.buttons["dayEditor.day.2"].firstMatch
        XCTAssertTrue(day3.waitForExistence(timeout: 5), "the split must add a day")
        XCTAssertTrue(app.buttons["dayEditor.day.1"].label.hasSuffix("transfer"), "the transfer stays with its day")

        // The new end moves by a drag on the profile.
        let split = day1.label
        let start = profileHandle(app, "Day 1 end").coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        start.press(forDuration: 0.2, thenDragTo: start.withOffset(CGVector(dx: -40, dy: 0)))
        XCTAssertNotEqual(day1.label, split, "the drag must move the day end")
        snap(app, "DE-edit-dragged")

        // Close in on the transfer at Devil's Lake: the stops show, and a callout ends Day 1 there.
        // Map pins sit beside the map element, not in it.
        let transfer = app.otherElements
            .matching(NSPredicate(format: "label == 'Day 2 end' AND identifier != 'profile.handle'")).firstMatch
        XCTAssertTrue(transfer.exists, "the transfer pin is missing on the map")
        let beside = transfer.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).withOffset(CGVector(dx: 0, dy: 30))
        beside.doubleTap()
        beside.doubleTap()
        let camp = app.otherElements
            .matching(NSPredicate(format: "label BEGINSWITH %@", "Devil's Lake State Park Campgrounds")).firstMatch
        XCTAssertTrue(camp.waitForExistence(timeout: 10), "zoomed in, the map shows the stops")
        camp.tap()
        let endHere = app.buttons["map.stopAction"]
        XCTAssertTrue(endHere.waitForExistence(timeout: 5), "the callout has no button")
        XCTAssertEqual(endHere.label, "End Day 1 here", "the nearest end that may move is Day 1's")
        snap(app, "DE-edit-callout")
        endHere.tap()
        let named = NSPredicate(format: "label CONTAINS %@", "Devil's Lake State Park Campgrounds")
        XCTAssertEqual(
            XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: named, object: day1)], timeout: 5), .completed,
            "the day takes the stop's name: \(day1.label)")
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

    /// At the middle height a swipe on the day list scrolls the list and leaves the sheet where
    /// it is; the grab handle still moves the sheet, down to the one-line summary.
    @MainActor
    func testTheDayListScrollsAtTheMiddleHeight() {
        let app = launch(fixtures: "trips", importSample: "gpx")
        let row = app.buttons["import.startTrip"]
        XCTAssertTrue(row.waitForExistence(timeout: 10), "import Start a trip row missing")
        row.tap()
        let increment = app.buttons["dayEditor.days-Increment"]
        XCTAssertTrue(increment.waitForExistence(timeout: 10), "the day editor is missing")
        for _ in 0..<7 { increment.tap() }
        let first = app.buttons["dayEditor.day.0"].firstMatch
        let summary = app.staticTexts["dayEditor.summary"]
        XCTAssertTrue(summary.label.hasPrefix("8 days"), "eight days: \(summary.label)")
        let headerY = summary.frame.minY
        let last = app.buttons["dayEditor.day.7"].firstMatch
        XCTAssertFalse(last.exists && last.isHittable, "the last day starts below the fold")

        first.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
            .press(forDuration: 0.05, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.45)))
        XCTAssertEqual(summary.frame.minY, headerY, accuracy: 2, "the sheet stays at the middle height")
        XCTAssertTrue(last.waitForExistence(timeout: 2) && last.isHittable, "the list scrolls to the last day")
        snap(app, "DE-list-scrolled")

        let grabber = app.buttons["Sheet Grabber"].firstMatch
        grabber.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
            .press(forDuration: 0.1, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.99)))
        XCTAssertTrue(summary.waitForExistence(timeout: 2))
        XCTAssertGreaterThan(summary.frame.minY, headerY + 100, "the grab handle lowers the sheet")
        Thread.sleep(forTimeInterval: 1)
        snap(app, "DE-peek-split")
    }

    /// A day end's drag band on the profile.
    @MainActor
    private func profileHandle(_ app: XCUIApplication, _ name: String) -> XCUIElement {
        let handle = app.otherElements.matching(NSPredicate(format: "identifier == 'profile.handle' AND label == %@", name)).firstMatch
        XCTAssertTrue(handle.waitForExistence(timeout: 5), "the profile handle \(name) is missing")
        return handle
    }
}
