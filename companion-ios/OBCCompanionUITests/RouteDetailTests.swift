import XCTest

/// The three detail dressings (planned / tracked / import landing), the
/// waypoints push, rename, the delete-confirm path, and the upload seam.
/// Host-side logic lives in `RouteDetailModelTests`; this proves the
/// navigation wiring end to end — including the real GPX decoder on the
/// bundled Komoot sample.
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
        // Pin the locale: `OBCFormat` localizes numbers ("62,4 km" on a German
        // sim), and these tests assert the design's en-US strings.
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

    /// Land on the planned detail for the Kettle Moraine fixture route.
    @MainActor
    private func openPlannedDetail(_ app: XCUIApplication) {
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
        let card = app.buttons["main.card.kettle-moraine-loop"]
        XCTAssertTrue(card.waitForExistence(timeout: 10))
        card.tap()
        XCTAssertTrue(app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5), "detail missing")
    }

    // MARK: Planned

    /// Hero + stat strip + waypoints row + profile + inline actions.
    @MainActor
    func testPlannedDetailShowsTheProfileLayout() {
        let app = launch()
        openPlannedDetail(app)

        XCTAssertTrue(app.staticTexts["Kettle Moraine Loop"].waitForExistence(timeout: 5))
        // A stat renders value+unit as one text element: "62.4 km".
        XCTAssertTrue(app.staticTexts["62.4 km"].exists, "distance stat missing")
        XCTAssertTrue(app.staticTexts["3:20"].exists, "est. time stat missing")
        // MAX derives from the saved record's geometry (library-first) —
        // pin the shape ("N %"), not a fixture constant.
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

    /// The disclosure pushes the waypoint list in ride order.
    @MainActor
    func testWaypointsRowPushesW1() {
        let app = launch()
        openPlannedDetail(app)

        let waypointsRow = app.buttons["detail.waypoints"]
        XCTAssertTrue(waypointsRow.waitForExistence(timeout: 5))
        waypointsRow.tap()

        XCTAssertTrue(app.descendants(matching: .any)["waypoints.screen"].firstMatch.waitForExistence(timeout: 5), "waypoints screen missing")
        XCTAssertTrue(app.staticTexts["Ottawa Lake trailhead"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Emma Carlin junction"].exists)
        snap(app, "W1-waypoints")

        app.navigationBars.buttons.firstMatch.tap()  // back to the detail
        XCTAssertTrue(app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5))
    }

    /// Pencil → rename alert → the title and the list row both update.
    @MainActor
    func testRenameUpdatesTitleAndList() {
        let app = launch()
        openPlannedDetail(app)

        app.buttons["detail.rename"].tap()
        let alert = app.alerts["Rename route"]
        XCTAssertTrue(alert.waitForExistence(timeout: 5), "rename alert missing")
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

    /// Delete → confirm → pops back with the row gone.
    @MainActor
    func testDeleteRoutesThroughH1AndPops() {
        let app = launch()
        openPlannedDetail(app)

        app.buttons["detail.delete"].tap()
        // Scoped to the sheet — the inline action shares the "Delete route" label.
        let confirm = app.sheets.buttons["Delete route"]
        XCTAssertTrue(confirm.waitForExistence(timeout: 5), "delete confirm missing")
        snap(app, "H1-delete-from-detail")
        confirm.tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5), "should pop to the list")
        XCTAssertFalse(app.buttons["main.card.kettle-moraine-loop"].exists, "deleted row still listed")
        XCTAssertTrue(app.staticTexts["Sugar River Trail"].exists, "other rows must survive")
    }

    // The upload action's sheet is covered end to end in `UploadSheetTests`
    // (progress / done / interrupted / cancel / save-on-upload).

    // MARK: Tracked

    /// Ride stats, the tracked tag, and the coming-soon services block.
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

        XCTAssertTrue(app.staticTexts["58.2 km"].waitForExistence(timeout: 5), "ride distance stat missing")
        XCTAssertTrue(app.staticTexts["2:51"].exists, "moving-time stat missing")
        XCTAssertTrue(app.staticTexts["Strava"].exists, "services block missing")
        XCTAssertTrue(app.staticTexts["Komoot"].exists)
        XCTAssertTrue(app.buttons["detail.rename"].exists, "tracked name must stay editable")
        snap(app, "E3-ride-detail")
    }

    /// Delete → confirm → pops back with the ride gone (phone-side only; the
    /// device keeps its copy).
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
        XCTAssertTrue(delete.waitForExistence(timeout: 5), "tracked delete missing")
        // The actions sit at the end of the scroll, below the services block.
        for _ in 0..<4 where !delete.isHittable { app.swipeUp(velocity: .fast) }
        delete.tap()
        let confirm = app.sheets.buttons["Delete ride"]
        XCTAssertTrue(confirm.waitForExistence(timeout: 5), "delete confirm missing")
        snap(app, "H1-delete-ride-from-detail")
        confirm.tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5), "should pop to the list")
        XCTAssertFalse(card.exists, "deleted ride still listed")
    }

    // MARK: Import landing

    /// End to end off the real GPX decoder: source banner, unsaved tag,
    /// Points stat, waypoints-from-file, and Save to Planned landing.
    @MainActor
    func testImportSampleLandsOnE1AndSavesToPlanned() {
        let app = launch(importSample: true)

        XCTAssertTrue(app.otherElements["detail.importedFrom"].firstMatch.waitForExistence(timeout: 10)
                      || app.staticTexts["IMPORTED FROM KOMOOT"].waitForExistence(timeout: 5),
                      "import source banner missing")
        XCTAssertTrue(app.staticTexts["Schwarzwald Tour · Tag 2"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["CLIMB"].waitForExistence(timeout: 5), "climb stat missing")
        XCTAssertTrue(app.staticTexts["DESCENT"].waitForExistence(timeout: 5), "descent stat missing")

        let waypointsRow = app.buttons["detail.waypoints"]
        XCTAssertTrue(waypointsRow.exists, "waypoints-from-file row missing")
        XCTAssertTrue(app.buttons["detail.saveToPlanned"].exists)
        XCTAssertTrue(app.buttons["Cancel"].exists, "import landing must keep the Cancel escape")
        snap(app, "E1-import-landing")

        app.buttons["detail.saveToPlanned"].tap()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5), "save should dismiss E1")
        let savedRow = app.staticTexts["Schwarzwald Tour · Tag 2"]
        XCTAssertTrue(savedRow.waitForExistence(timeout: 5), "saved route must land in the Planned list")
        snap(app, "C1-after-import-save")

        // Reopening the saved route must keep the parsed waypoints + profile
        // (they live app-side — the device never had this route).
        savedRow.tap()
        XCTAssertTrue(app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["detail.waypoints"].waitForExistence(timeout: 5),
                      "saved import lost its waypoints")
        XCTAssertTrue(app.staticTexts["ELEVATION PROFILE"].exists, "saved import lost its profile")
        snap(app, "E2-saved-import")
    }

    /// Cancel discards — nothing lands in the library.
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
    /// Clear a text field by selecting all + deleting (no clear button in alerts).
    func clearText() {
        guard let current = value as? String, !current.isEmpty else { return }
        typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: current.count))
    }
}
