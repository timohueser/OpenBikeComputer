import XCTest

/// The upload sheet over the route detail: moving progress with a reachable Cancel, the done
/// confirm, the interrupted and resume path, and the import landing that saves on upload.
/// Host-side logic lives in `UploadSheetModelTests`.
final class UploadSheetTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch(scenario: String = "happyPath", importSample: Bool = false) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", scenario]
        if importSample { app.launchArguments += ["-OBCImportSample"] }
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
    private func startUpload(_ app: XCUIApplication) {
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
        let card = app.buttons["main.card.blue-mounds-backroads"]
        XCTAssertTrue(card.waitForExistence(timeout: 10))
        card.tap()
        let upload = app.buttons["detail.upload"]
        XCTAssertTrue(upload.waitForExistence(timeout: 5), "upload action missing")
        upload.tap()
    }

    /// Moving progress at a realistic speed, the done confirm, and the detail still underneath:
    /// the app never left it.
    @MainActor
    func testHappyPathUploadsThroughF2AndStaysOnTheRoute() {
        let app = launch()
        startUpload(app)

        XCTAssertTrue(app.staticTexts["Uploading to Trailhead"].waitForExistence(timeout: 5), "F title missing")
        XCTAssertTrue(app.buttons["upload.cancel"].exists, "Cancel must always be reachable")
        XCTAssertTrue(app.staticTexts["upload.percent"].exists, "percentage missing")
        snap(app, "F-uploading")

        XCTAssertTrue(app.staticTexts["On the device"].waitForExistence(timeout: 15), "F₂ missing")
        snap(app, "F2-uploaded-done")

        app.buttons["upload.done"].tap()
        XCTAssertFalse(app.staticTexts["On the device"].waitForExistence(timeout: 2), "sheet must dismiss")
        XCTAssertTrue(
            app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5),
            "must land back on the route detail"
        )
    }

    /// The transfer drops partway, surfaces the resume framing, and Resume carries it to the done
    /// confirm from the committed offset.
    @MainActor
    func testDropSurfacesInterruptedAndResumeFinishes() {
        let app = launch(scenario: "uploadDrop")
        startUpload(app)

        XCTAssertTrue(app.staticTexts["Upload interrupted"].waitForExistence(timeout: 15), "interrupted framing missing")
        XCTAssertTrue(app.buttons["upload.cancel"].exists, "Cancel must stay reachable when interrupted")
        snap(app, "F-interrupted")

        app.buttons["upload.resume"].tap()
        XCTAssertTrue(app.staticTexts["Uploading to Trailhead"].waitForExistence(timeout: 5), "resume must re-enter F")
        XCTAssertTrue(app.staticTexts["On the device"].waitForExistence(timeout: 15), "resumed upload must finish")
    }

    /// Cancel aborts and returns to the route detail: no confirm, no detour.
    @MainActor
    func testCancelAbortsBackToTheDetail() {
        let app = launch()
        startUpload(app)

        let cancel = app.buttons["upload.cancel"]
        XCTAssertTrue(cancel.waitForExistence(timeout: 5))
        cancel.tap()

        XCTAssertTrue(
            app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5),
            "cancel must land back on the detail"
        )
        XCTAssertFalse(app.staticTexts["On the device"].exists, "a canceled upload must never read as done")
        XCTAssertFalse(app.buttons["upload.cancel"].exists, "sheet must be gone")
    }

    /// Completing the upload from the import landing also saves the route: the cover closes and
    /// the route sits in Planned.
    @MainActor
    func testUploadFromImportLandingSavesToPlanned() {
        let app = launch(importSample: true)

        let upload = app.buttons["detail.upload"]
        XCTAssertTrue(upload.waitForExistence(timeout: 10), "E1 upload action missing")
        upload.tap()

        XCTAssertTrue(app.staticTexts["On the device"].waitForExistence(timeout: 15), "F₂ missing")
        app.buttons["upload.done"].tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 5), "landing must close after F₂")
        XCTAssertTrue(
            app.staticTexts["Schwarzwald Tour · Tag 2"].waitForExistence(timeout: 5),
            "uploaded import must land in the Planned list"
        )
    }
}
