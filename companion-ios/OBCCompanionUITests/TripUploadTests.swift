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

    /// A retention-capable device opens the trip upload on the Auto-delete confirm
    /// (epic #638) — tap **Upload trip** to start the queue.
    @MainActor
    private func confirmTripUpload(_ app: XCUIApplication) {
        let begin = app.buttons["tripUpload.begin"]
        XCTAssertTrue(begin.waitForExistence(timeout: 10), "the trip Auto-delete confirm is missing")
        begin.tap()
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
        confirmTripUpload(app)  // clear the Auto-delete confirm (epic #638)
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
        confirmTripUpload(app)  // clear the Auto-delete confirm (epic #638)

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
        confirmTripUpload(app)  // the precheck runs on the confirm tap (epic #638)

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
        confirmTripUpload(app)  // clear the Auto-delete confirm (epic #638)
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

    // MARK: Auto-delete confirm (epic #638)

    /// A retention-capable device opens the trip upload on the Auto-delete confirm:
    /// the row seeds from the app default ("After 2 weeks") and the picker changes
    /// the whole-trip level before any bytes.
    @MainActor
    func testWholeTripUploadShowsAutoDeleteConfirm() {
        let app = launch()
        openTrip(app)
        app.buttons["trip.upload"].tap()

        XCTAssertTrue(app.buttons["tripUpload.begin"].waitForExistence(timeout: 10),
                      "the trip pre-transfer confirm is missing")
        let value = app.staticTexts["tripUpload.autoDelete.value"]
        XCTAssertTrue(value.waitForExistence(timeout: 5), "the trip Auto-delete row is missing")
        XCTAssertEqual(value.label, "After 2 weeks", "the row must seed from the app default")
        snap(app, "TR8-trip-upload-auto-delete")

        // The picker changes the whole-trip level.
        app.buttons["tripUpload.autoDelete"].tap()
        let option = app.buttons["After 1 month"]
        XCTAssertTrue(option.waitForExistence(timeout: 5), "trip picker options missing")
        option.tap()
        XCTAssertEqual(
            app.staticTexts["tripUpload.autoDelete.value"].label, "After 1 month",
            "picking a level must update the trip row")
    }

    /// An old-firmware device (no expiry) skips the confirm/row entirely — the trip
    /// upload starts running straight away, exactly as before epic #638.
    @MainActor
    func testOldFirmwareTripUploadSkipsTheConfirm() {
        let app = launch(extraArgs: ["-OBCOldFirmware"])
        openTrip(app)
        app.buttons["trip.upload"].tap()

        XCTAssertTrue(
            app.staticTexts["tripUpload.stageLabel"].waitForExistence(timeout: 10),
            "old-firmware trip upload must start immediately")
        XCTAssertFalse(app.buttons["tripUpload.begin"].exists, "no pre-transfer confirm on old firmware")
        XCTAssertFalse(app.staticTexts["tripUpload.autoDelete.value"].exists,
                       "no Auto-delete row on old firmware")
    }
}
