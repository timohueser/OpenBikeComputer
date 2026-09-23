import XCTest

/// The three detail dressings, the waypoints dropdown, rename, the delete path and the upload
/// seam, driven through the real UI against the mock. Host-side logic lives in
/// `RouteDetailModelTests`; this proves the navigation wiring end to end, including the real GPX
/// decoder on the bundled sample.
final class RouteDetailTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch(scenario: String = "happyPath", importSample: Bool = false) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", scenario]
        if importSample { app.launchArguments += ["-OBCImportSample"] }
        // Pin the locale: the formatter localizes numbers, and these tests assert English strings.
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

    /// Land on the detail for the fixture route.
    @MainActor
    private func openPlannedDetail(_ app: XCUIApplication) {
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
        let card = app.buttons["main.card.kettle-moraine-loop"]
        XCTAssertTrue(card.waitForExistence(timeout: 10))
        card.tap()
        XCTAssertTrue(app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5), "detail missing")
    }

    // MARK: Planned dressing

    /// Hero, stat strip, waypoints row, profile and inline actions.
    @MainActor
    func testPlannedDetailShowsTheProfileLayout() {
        let app = launch()
        openPlannedDetail(app)

        XCTAssertTrue(app.staticTexts["Kettle Moraine Loop"].waitForExistence(timeout: 5))
        // A stat renders its value and unit as one text element.
        XCTAssertTrue(app.staticTexts["62.4 km"].exists, "distance stat missing")
        XCTAssertTrue(app.staticTexts["3:12"].exists, "est. time stat missing")
        // The max grade derives from the saved record's geometry, so pin the shape and not a
        // fixture constant.
        let maxGrade = app.staticTexts.matching(
            NSPredicate(format: "label MATCHES %@", "\\d+ %")
        ).firstMatch
        XCTAssertTrue(maxGrade.waitForExistence(timeout: 5), "max grade stat missing")

        let waypointsRow = app.buttons["detail.waypoints"]
        XCTAssertTrue(waypointsRow.waitForExistence(timeout: 5), "waypoints disclosure missing")
        XCTAssertTrue(app.buttons["detail.upload"].exists, "upload action missing")
        XCTAssertTrue(app.buttons["detail.delete"].exists, "delete action missing")
        snap(app, "E2-route-detail")
    }

    /// The disclosure folds the waypoint list out in place, and folds it back on a second tap.
    /// There is no pushed screen.
    @MainActor
    func testWaypointsRowExpandsAndCollapsesInline() {
        let app = launch()
        openPlannedDetail(app)

        let waypointsRow = app.buttons["detail.waypoints"]
        XCTAssertTrue(waypointsRow.waitForExistence(timeout: 5))
        waypointsRow.tap()

        XCTAssertTrue(app.staticTexts["Ottawa Lake trailhead"].waitForExistence(timeout: 5), "W1 rows missing")
        XCTAssertTrue(app.staticTexts["Emma Carlin junction"].exists)
        // Still the detail screen: the dropdown must not navigate.
        XCTAssertTrue(app.descendants(matching: .any)["detail.screen"].firstMatch.exists)
        snap(app, "W1-waypoints-expanded")

        waypointsRow.tap()  // collapse
        XCTAssertTrue(waitForDisappearance(app.staticTexts["Ottawa Lake trailhead"]), "dropdown should fold back")
    }

    /// The dropdown collapse has an animation window, so poll briefly.
    @MainActor
    private func waitForDisappearance(_ element: XCUIElement, timeout: TimeInterval = 5) -> Bool {
        let expectation = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "exists == false"), object: element
        )
        return XCTWaiter().wait(for: [expectation], timeout: timeout) == .completed
    }

    /// The pencil opens a rename alert, and the title and the list row both update.
    @MainActor
    func testRenameUpdatesTitleAndList() {
        let app = launch()
        openPlannedDetail(app)

        app.buttons["detail.rename"].tap()
        let alert = app.alerts["Rename route"]
        XCTAssertTrue(alert.waitForExistence(timeout: 5), "H12 alert missing")
        snap(app, "H12-rename-route")

        let field = alert.textFields.firstMatch
        field.tap()
        field.clearText()
        field.typeText("Kettle Gravel Day")
        alert.buttons["Save"].tap()

        XCTAssertTrue(app.staticTexts["Kettle Gravel Day"].waitForExistence(timeout: 5), "title kept the old name")

        app.navigationBars.buttons.firstMatch.tap()  // back to the list
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Kettle Gravel Day"].waitForExistence(timeout: 5), "list row kept the old name")
    }

    /// Delete asks for confirmation, then pops back with the row gone.
    @MainActor
    func testDeleteRoutesThroughH1AndPops() {
        let app = launch()
        openPlannedDetail(app)

        let delete = app.buttons["detail.delete"]
        XCTAssertTrue(delete.waitForExistence(timeout: 5), "delete action missing")
        for _ in 0..<4 where !delete.isHittable { app.swipeUp(velocity: .fast) }
        delete.tap()
        // Scoped to the sheet: the inline action shares the "Delete route" label.
        let confirm = app.sheets.buttons["Delete route"]
        XCTAssertTrue(confirm.waitForExistence(timeout: 5), "H1 confirm missing")
        snap(app, "H1-delete-from-detail")
        confirm.tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5), "should pop to the list")
        XCTAssertFalse(app.buttons["main.card.kettle-moraine-loop"].exists, "deleted row still listed")
        XCTAssertTrue(app.staticTexts["Sugar River Trail"].exists, "other rows must survive")
    }

    // The upload action's sheet is covered end to end in `UploadSheetTests`.

    // MARK: Tracked dressing

    /// The ride's stats line, the tracked tag, and the coming-soon services block.
    @MainActor
    func testTrackedDetailShowsRideStatsAndServices() {
        let app = launch()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        app.buttons["Tracked"].tap()
        // Tracked is library-first: sync to pull the ride in first.
        app.buttons["topbar.sync"].tap()

        let card = app.buttons["main.card.ride-kettle-moraine"]
        XCTAssertTrue(card.waitForExistence(timeout: 30))
        card.tap()
        XCTAssertTrue(app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5))

        let stats = app.staticTexts["detail.statsLine"]
        XCTAssertTrue(stats.waitForExistence(timeout: 5), "ride stats line missing")
        XCTAssertTrue(stats.label.hasPrefix("58.2 km · 2:51 · "), "distance and moving time lead: \(stats.label)")
        XCTAssertTrue(app.staticTexts["Strava"].exists, "services block missing")
        XCTAssertTrue(app.staticTexts["Komoot"].exists)
        XCTAssertTrue(app.buttons["detail.rename"].exists, "E3 name must stay editable")
        snap(app, "E3-ride-detail")
    }

    /// Delete asks for confirmation, then pops back with the ride gone. Phone-side only: the
    /// device keeps its copy.
    @MainActor
    func testTrackedDeleteRoutesThroughH1AndPops() {
        let app = launch()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        app.buttons["Tracked"].tap()
        // Tracked is library-first: sync to pull the ride in first.
        app.buttons["topbar.sync"].tap()

        let card = app.buttons["main.card.ride-kettle-moraine"]
        XCTAssertTrue(card.waitForExistence(timeout: 30))
        card.tap()

        let delete = app.buttons["detail.delete"]
        XCTAssertTrue(delete.waitForExistence(timeout: 5), "E3 delete missing")
        // The actions sit at the end of the scroll, below the services block.
        for _ in 0..<4 where !delete.isHittable { app.swipeUp(velocity: .fast) }
        delete.tap()
        let confirm = app.sheets.buttons["Delete ride"]
        XCTAssertTrue(confirm.waitForExistence(timeout: 5), "H1 confirm missing")
        snap(app, "H1-delete-ride-from-detail")
        confirm.tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5), "should pop to the list")
        XCTAssertFalse(card.exists, "deleted ride still listed")
    }

    // MARK: Import landing

    /// The landing end to end off the real GPX decoder: source banner, unsaved tag, the points
    /// stat, waypoints from the file, and Save landing the route in the list.
    @MainActor
    func testImportSampleLandsOnE1AndSavesToPlanned() {
        let app = launch(importSample: true)

        XCTAssertTrue(app.otherElements["detail.importedFrom"].firstMatch.waitForExistence(timeout: 10)
                      || app.staticTexts["IMPORTED FROM KOMOOT"].waitForExistence(timeout: 5),
                      "E1 source banner missing")
        XCTAssertTrue(app.staticTexts["Schwarzwald Tour · Tag 2"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["CLIMB"].waitForExistence(timeout: 5), "climb stat missing")
        XCTAssertTrue(app.staticTexts["DESCENT"].waitForExistence(timeout: 5), "descent stat missing")

        let waypointsRow = app.buttons["detail.waypoints"]
        XCTAssertTrue(waypointsRow.exists, "waypoints-from-file row missing")
        XCTAssertTrue(app.buttons["import.newRoute"].exists)
        XCTAssertTrue(app.buttons["Cancel"].exists, "E1 must keep the Cancel escape")
        snap(app, "E1-import-landing")

        app.buttons["import.newRoute"].tap()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5), "save should dismiss E1")
        let savedRow = app.staticTexts["Schwarzwald Tour · Tag 2"]
        XCTAssertTrue(savedRow.waitForExistence(timeout: 5), "saved route must land in the Planned list")
        snap(app, "C1-after-import-save")

        // Reopening the saved route must keep the parsed waypoints and profile: they live
        // app-side, because the device never had this route.
        savedRow.tap()
        XCTAssertTrue(app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["detail.waypoints"].waitForExistence(timeout: 5),
                      "saved import lost its waypoints")
        XCTAssertTrue(app.staticTexts["ELEVATION PROFILE"].exists, "saved import lost its profile")
        snap(app, "E2-saved-import")
    }

    /// The pencil works on the landing itself, so the route saves under the new name with no
    /// save-then-reopen round trip.
    @MainActor
    func testImportRenamesOnTheLandingAndSavesUnderTheNewName() {
        let app = launch(importSample: true)

        let rename = app.buttons["detail.rename"]
        XCTAssertTrue(rename.waitForExistence(timeout: 10), "E1 must offer the rename pencil")
        rename.tap()
        let alert = app.alerts["Rename route"]
        XCTAssertTrue(alert.waitForExistence(timeout: 5), "H12 alert missing on E1")

        let field = alert.textFields.firstMatch
        // Tap past the text's right end: a centre tap lands the caret mid-name, and clearing only
        // deletes backwards.
        field.coordinate(withNormalizedOffset: CGVector(dx: 0.98, dy: 0.5)).tap()
        field.clearText()
        field.typeText("Schwarzwald Gravel")
        alert.buttons["Save"].tap()

        XCTAssertTrue(app.staticTexts["Schwarzwald Gravel"].waitForExistence(timeout: 5), "E1 title kept the old name")
        snap(app, "E1-renamed")

        app.buttons["import.newRoute"].tap()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Schwarzwald Gravel"].waitForExistence(timeout: 5),
                      "the renamed import must land in Planned under the new name")
    }

    /// Cancel discards: nothing lands in the library.
    @MainActor
    func testImportCancelDiscards() {
        let app = launch(importSample: true)
        let cancel = app.buttons["Cancel"]
        XCTAssertTrue(cancel.waitForExistence(timeout: 10))
        cancel.tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.staticTexts["Schwarzwald Tour · Tag 2"].exists, "cancelled import must not save")
    }
}

extension XCUIElement {
    /// Clear a text field by selecting all and deleting; alerts have no clear button.
    func clearText() {
        guard let current = value as? String, !current.isEmpty else { return }
        typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: current.count))
    }
}
