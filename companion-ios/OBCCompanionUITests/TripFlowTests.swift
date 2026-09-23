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
    /// opens the trip page.
    @MainActor
    func testImportStartsATrip() {
        let app = launch(fixtures: "trips", importSample: "gpx")

        let row = app.buttons["import.startTrip"]
        XCTAssertTrue(row.waitForExistence(timeout: 10), "import Start a trip row missing")
        XCTAssertTrue(app.buttons["import.addToTrip"].exists, "the one fixture trip must be offered")
        snap(app, "TR7-import-rows")
        row.tap()

        XCTAssertTrue(
            app.descendants(matching: .any)["trip.day.0"].firstMatch.waitForExistence(timeout: 10),
            "Start a trip must open the new trip's page")
        XCTAssertTrue(app.navigationBars["Schwarzwald Tour · Tag 2"].exists, "the trip takes the route's name")
        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'main.card.'"))
            .matching(NSPredicate(format: "label CONTAINS 'Schwarzwald'")).firstMatch.exists,
            "the imported route must not also be a route card")
    }

    /// A trip is a Planned row of its own: with no loose route left, the list still shows the
    /// trip card, not the empty state.
    @MainActor
    func testATripWithoutLooseRoutesStaysListed() {
        let app = launch(fixtures: "empty", importSample: "gpx")

        let row = app.buttons["import.startTrip"]
        XCTAssertTrue(row.waitForExistence(timeout: 10), "import Start a trip row missing")
        row.tap()
        XCTAssertTrue(
            app.descendants(matching: .any)["trip.day.0"].firstMatch.waitForExistence(timeout: 10),
            "Start a trip must open the new trip's page")
        app.navigationBars.buttons.element(boundBy: 0).tap()

        waitForMain(app)
        let cards = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'main.trip.'"))
        XCTAssertTrue(cards.firstMatch.waitForExistence(timeout: 5), "the trip card is missing")
        XCTAssertEqual(cards.count, 1, "one trip must show one card")
        XCTAssertFalse(app.staticTexts["No planned routes yet"].exists, "a trip is not an empty list")
        snap(app, "trip-only-list")
    }
}
