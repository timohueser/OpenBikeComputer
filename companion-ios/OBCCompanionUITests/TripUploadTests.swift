import XCTest

/// TR8 acceptance on the simulator: whole-trip upload driven through the real UI
/// against the `trips` fixture. The queue-planner, adoption, and reconcile logic
/// are host-tested (`TripUploadModelTests` / `TripReconcileModelTests`); this
/// proves the wiring — Upload trip → queued sheet → done, the interrupt/resume
/// framing, the storage-precheck failure, and delete-trip-&-routes.
final class TripUploadTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch(scenario: String = "happyPath", extraArgs: [String] = []) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", scenario, "-OBCFixtures", "trips"]
        app.launchArguments += ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launchArguments += extraArgs
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

    private let tripCardID = "main.trip.driftless-weekender"
    private let stageAID = "trip.stage.devils-lake-overnighter"

    @MainActor
    private func openTrip(_ app: XCUIApplication) {
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
        let card = app.buttons[tripCardID]
        XCTAssertTrue(card.waitForExistence(timeout: 10), "trip card missing")
        card.tap()
        XCTAssertTrue(app.buttons[stageAID].waitForExistence(timeout: 10), "trip page did not open")
    }

    // MARK: Happy path

    /// Upload trip → the queued sheet walks the stages then the trip object, and
    /// lands on the "Trip on the device" confirm.
    @MainActor
    func testWholeTripUploadHappyPath() {
        let app = launch()
        openTrip(app)

        let upload = app.buttons["trip.upload"]
        XCTAssertTrue(upload.isEnabled, "Upload trip disabled")
        upload.tap()

        let sheet = app.otherElements["tripUpload.sheet"]
        XCTAssertTrue(sheet.waitForExistence(timeout: 10), "trip upload sheet missing")
        // The queued-mode header appears while stages move.
        XCTAssertTrue(
            app.staticTexts["tripUpload.stageLabel"].waitForExistence(timeout: 10),
            "queued-mode stage header missing")
        snap(app, "TR8-trip-upload-queued")

        // It reaches the done confirm.
        XCTAssertTrue(
            app.staticTexts["tripUpload.doneTitle"].waitForExistence(timeout: 20),
            "trip upload never completed")
        XCTAssertTrue(app.staticTexts["tripUpload.doneTally"].exists, "done tally missing")
        snap(app, "TR8-trip-upload-done")
        app.buttons["tripUpload.done"].tap()

        // Back on the trip page.
        XCTAssertTrue(app.buttons[stageAID].waitForExistence(timeout: 10), "did not return to trip page")
    }

    // MARK: Interrupt + resume

    /// A mid-upload drop swaps in the interrupted framing; Resume restarts the
    /// current stage and the trip still lands.
    @MainActor
    func testWholeTripUploadInterruptThenResume() {
        // Arm the next transfer to drop partway through (the first stage).
        let app = launch(scenario: "uploadDrop")
        openTrip(app)
        app.buttons["trip.upload"].tap()

        let resume = app.buttons["tripUpload.resume"]
        XCTAssertTrue(resume.waitForExistence(timeout: 15), "interrupted framing never appeared")
        snap(app, "TR8-trip-upload-interrupted")
        resume.tap()

        XCTAssertTrue(
            app.staticTexts["tripUpload.doneTitle"].waitForExistence(timeout: 25),
            "trip upload did not finish after resume")
        app.buttons["tripUpload.done"].tap()
    }

    // MARK: Storage precheck (fails before any bytes)

    /// The device is nearly full — the precheck fails upfront with the storage
    /// guidance, never a partial upload.
    @MainActor
    func testWholeTripUploadStoragePrecheckFailure() {
        let app = launch(extraArgs: ["-OBCDeviceRoutesFull"])
        openTrip(app)
        app.buttons["trip.upload"].tap()

        XCTAssertTrue(
            app.staticTexts["tripUpload.failedTitle"].waitForExistence(timeout: 10),
            "precheck failure card missing")
        XCTAssertTrue(
            app.staticTexts["Device storage full"].exists,
            "storage-full title missing")
        snap(app, "TR8-trip-upload-precheck-fail")
        app.buttons["tripUpload.close"].tap()
    }

    // MARK: Delete trip & routes while connected

    /// Upload the trip, then Delete trip & routes — the trip and its members are
    /// gone from the library; a loose route survives.
    @MainActor
    func testDeleteTripAndRoutesAfterUpload() {
        let app = launch()
        openTrip(app)

        // Land it on the device first.
        app.buttons["trip.upload"].tap()
        XCTAssertTrue(
            app.staticTexts["tripUpload.doneTitle"].waitForExistence(timeout: 20),
            "trip upload never completed")
        app.buttons["tripUpload.done"].tap()
        XCTAssertTrue(app.buttons[stageAID].waitForExistence(timeout: 10), "did not return to trip page")

        // Delete trip & routes.
        app.buttons["trip.overflow"].tap()
        let delete = app.buttons["trip.delete"]
        XCTAssertTrue(delete.waitForExistence(timeout: 5), "overflow menu did not open")
        delete.tap()
        app.sheets.buttons["Delete trip & routes"].tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons[tripCardID].waitForExistence(timeout: 3), "trip card survived delete")
        XCTAssertFalse(app.staticTexts["Devil's Lake Overnighter"].exists, "member route survived delete")
        XCTAssertTrue(app.staticTexts["Kettle Moraine Loop"].exists, "loose route wrongly removed")
    }
}
