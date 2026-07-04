import XCTest

/// B6 acceptance on the simulator: TCX through the real decoder onto E1, the
/// H5 unsupported-file alert, and H4 (a share arriving before pairing — the
/// route saves, upload waits). The GPX E1 walk lives in `RouteDetailTests`;
/// decoder logic is host-tested in `TCXRouteDecoderTests`.
final class ImportTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch(scenario: String = "happyPath", importSample: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", scenario]
        app.launchArguments += ["-OBCImportSample", importSample]
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

    // MARK: TCX → E1

    /// A TCX course lands on E1 through the real decoder — name, author
    /// banner, and the CoursePoint waypoints in ride order (W1).
    @MainActor
    func testTCXImportLandsOnE1WithCourseWaypoints() {
        let app = launch(importSample: "tcx")

        XCTAssertTrue(app.staticTexts["Alpe d'Huez Climb"].waitForExistence(timeout: 10), "TCX course name missing")
        XCTAssertTrue(app.staticTexts["IMPORTED FROM GARMIN"].waitForExistence(timeout: 5),
                      "TCX author banner missing")

        let waypointsRow = app.buttons["detail.waypoints"]
        XCTAssertTrue(waypointsRow.waitForExistence(timeout: 5), "CoursePoint waypoints row missing")
        snap(app, "E1-import-tcx")

        waypointsRow.tap()  // fold the dropdown out in place
        XCTAssertTrue(app.staticTexts["Turn 21"].waitForExistence(timeout: 5), "first course point missing")
        XCTAssertTrue(app.staticTexts["Summit"].exists, "last course point missing")
        snap(app, "W1-tcx-coursepoints")
    }

    /// Saving a TCX import lands it in Planned, like any other route file.
    @MainActor
    func testTCXImportSavesToPlanned() {
        let app = launch(importSample: "tcx")

        let save = app.buttons["detail.saveToPlanned"]
        XCTAssertTrue(save.waitForExistence(timeout: 10))
        save.tap()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Alpe d'Huez Climb"].waitForExistence(timeout: 5),
                      "saved TCX route must land in the Planned list")
    }

    // MARK: H5 · unsupported file

    /// Anything that isn't GPX/TCX gets the plain H5 alert, and nothing imports.
    @MainActor
    func testUnsupportedFileShowsH5() {
        let app = launch(importSample: "bad")

        let alert = app.alerts["Couldn't read that file"]
        XCTAssertTrue(alert.waitForExistence(timeout: 10), "H5 alert missing")
        XCTAssertTrue(alert.staticTexts["OBC imports GPX and TCX route files. That one looked like something else."].exists,
                      "H5 copy must name the accepted formats")
        snap(app, "H5-unsupported-file")

        alert.buttons["OK"].tap()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5), "app must carry on after H5")
        XCTAssertFalse(app.descendants(matching: .any)["detail.screen"].firstMatch.exists, "nothing must import")
    }

    // MARK: H4 · import with no device paired

    /// A share arriving before pairing presents E1 over the pairing intro with
    /// the H4 framing: no-device banner, Save to Planned, Pair a device — and
    /// no Upload (there is nothing to upload to).
    @MainActor
    func testImportWithNoDeviceShowsH4Framing() {
        let app = launch(scenario: "noDevice", importSample: "gpx")

        XCTAssertTrue(app.staticTexts["Schwarzwald Tour · Tag 2"].waitForExistence(timeout: 10), "E1 must present over D1")
        // The banner renders title+message as one combined Text — match by fragment.
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'No device paired yet'"))
                          .firstMatch.waitForExistence(timeout: 5),
                      "H4 banner missing")
        XCTAssertTrue(app.buttons["detail.saveToPlanned"].exists)
        XCTAssertTrue(app.buttons["detail.pairDevice"].exists)
        XCTAssertFalse(app.buttons["detail.upload"].exists, "H4 must not offer Upload")
        snap(app, "H4-import-no-device")

        // Save to Planned returns to where the share interrupted — the D1 intro.
        app.buttons["detail.saveToPlanned"].tap()
        XCTAssertTrue(app.staticTexts["pair.introTitle"].firstMatch.waitForExistence(timeout: 5)
                      || app.buttons["pair.start"].waitForExistence(timeout: 5),
                      "saving without a device should land back on D1")
    }

    /// H4 "Pair a device" keeps the route (saves it) and drops into the D2
    /// scan; after pairing completes, the imported route is in Planned.
    @MainActor
    func testH4PairADeviceSavesAndPairsThroughToThePlannedList() {
        let app = launch(scenario: "noDevice", importSample: "gpx")

        let pair = app.buttons["detail.pairDevice"]
        XCTAssertTrue(pair.waitForExistence(timeout: 10))
        pair.tap()

        // D2 — the scan the button started; the mock finds the device.
        let row = app.buttons["pair.deviceRow"]
        XCTAssertTrue(row.waitForExistence(timeout: 15), "scan should surface the device row")
        row.tap()

        let goToRoutes = app.buttons["pair.goToRoutes"]
        XCTAssertTrue(goToRoutes.waitForExistence(timeout: 10), "pairing should complete (D4)")
        goToRoutes.tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["Schwarzwald Tour · Tag 2"].waitForExistence(timeout: 10),
                      "the H4-saved route must survive pairing + the device list load")
        snap(app, "C1-after-h4-pairing")
    }
}
