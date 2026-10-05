import XCTest

/// TCX through the real decoder onto the saved route, the unsupported-file alert, and a share
/// arriving before pairing, where the route saves and the upload waits. The GPX walk lives in
/// `RouteDetailTests`, and decoder logic is host-tested in `TCXRouteDecoderTests`.
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

    /// Keep the file's line on the keep-or-plan choice.
    @MainActor
    private func keepTheLine(_ app: XCUIApplication) {
        let keep = app.buttons["confirm.action.0"]
        XCTAssertTrue(keep.waitForExistence(timeout: 10), "the keep-or-plan choice is missing")
        keep.tap()
    }

    // MARK: TCX import

    /// A TCX course saves through the real decoder and its route page opens: name, author line,
    /// and the course-point waypoints in ride order. The route is in Planned.
    @MainActor
    func testTCXImportSavesAndOpensWithCourseWaypoints() {
        let app = launch(importSample: "tcx")
        keepTheLine(app)

        XCTAssertTrue(app.staticTexts["Alpe d'Huez Climb"].waitForExistence(timeout: 10), "TCX course name missing")
        XCTAssertTrue(app.staticTexts["Imported from Garmin"].waitForExistence(timeout: 5),
                      "TCX author line missing")

        let waypointsRow = app.buttons["detail.waypoints"]
        XCTAssertTrue(waypointsRow.waitForExistence(timeout: 5), "CoursePoint waypoints row missing")
        snap(app, "E2-import-tcx")

        waypointsRow.tap()  // fold the dropdown out in place
        XCTAssertTrue(app.staticTexts["Turn 21"].waitForExistence(timeout: 5), "first course point missing")
        XCTAssertTrue(app.staticTexts["Summit"].exists, "last course point missing")
        snap(app, "W1-tcx-coursepoints")

        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Alpe d'Huez Climb"].waitForExistence(timeout: 5),
                      "saved TCX route must be in the Planned list")
    }

    // MARK: Unsupported file

    /// Anything that is not GPX or TCX gets the plain alert, and nothing imports.
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

    // MARK: Import with no device paired

    /// A share arriving before pairing asks keep-or-plan over the welcome screen and saves the
    /// route there; the library has it.
    @MainActor
    func testImportWithNoDeviceSavesBeforePairing() {
        let app = launch(scenario: "noDevice", importSample: "gpx")
        keepTheLine(app)

        let browse = app.buttons["onboarding.browse"]
        XCTAssertTrue(browse.waitForExistence(timeout: 5), "saving without a device must land back on the welcome screen")
        snap(app, "H4-import-no-device")
        browse.tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["Schwarzwald Tour · Tag 2"].waitForExistence(timeout: 10),
                      "the route saved before pairing must be in the library")
    }
}
