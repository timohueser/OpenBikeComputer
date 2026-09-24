import XCTest

/// Whole-trip upload driven through the real UI against the trips fixture. The queue planner and
/// reconcile logic are host-tested; this proves the wiring: Send, the queued sheet, the device
/// screen confirm, the interrupt and resume framing, the capacity boundary, and delete trip.
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
    private let dayAID = "trip.day.0"

    @MainActor
    private func openTrip(_ app: XCUIApplication) {
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
        let card = app.buttons[tripCardID]
        XCTAssertTrue(card.waitForExistence(timeout: 10), "trip card missing")
        card.tap()
        XCTAssertTrue(app.descendants(matching: .any)[dayAID].waitForExistence(timeout: 10), "trip page did not open")
    }

    // MARK: Happy path

    /// The queued sheet walks the day routes then the trip object, and lands on the done confirm.
    @MainActor
    func testWholeTripUploadHappyPath() {
        let app = launch()
        openTrip(app)

        let upload = app.buttons["trip.upload"]
        XCTAssertTrue(upload.isEnabled, "Upload trip disabled")
        upload.tap()

        let sheet = app.descendants(matching: .any)["tripUpload.sheet"].firstMatch
        XCTAssertTrue(sheet.waitForExistence(timeout: 10), "trip upload sheet missing")
        // The queued-mode header appears while day routes move.
        XCTAssertTrue(
            app.staticTexts["tripUpload.stepLabel"].waitForExistence(timeout: 10),
            "queued-mode step header missing")
        snap(app, "TR8-trip-upload-queued")

        // It reaches the done confirm.
        XCTAssertTrue(
            app.staticTexts["upload.doneTitle"].waitForExistence(timeout: 20),
            "trip upload never completed")
        XCTAssertTrue(app.images["upload.deviceScreen"].exists, "the device drawing is missing")
        snap(app, "TR8-trip-upload-done")
        app.buttons["upload.done"].tap()

        // Back on the trip page.
        XCTAssertTrue(app.descendants(matching: .any)[dayAID].waitForExistence(timeout: 10), "did not return to trip page")
    }

    // MARK: Interrupt + resume

    /// A mid-upload drop swaps in the interrupted framing; Resume restarts the current step and
    /// the trip still lands.
    @MainActor
    func testWholeTripUploadInterruptThenResume() {
        // Arm the next transfer to drop partway through the first day route.
        let app = launch(scenario: "uploadDrop")
        openTrip(app)
        app.buttons["trip.upload"].tap()

        let resume = app.buttons["tripUpload.resume"]
        XCTAssertTrue(resume.waitForExistence(timeout: 15), "interrupted framing never appeared")
        snap(app, "TR8-trip-upload-interrupted")
        resume.tap()

        XCTAssertTrue(
            app.staticTexts["upload.doneTitle"].waitForExistence(timeout: 25),
            "trip upload did not finish after resume")
        app.buttons["upload.done"].tap()
    }

    // MARK: Menu capacity is not storage capacity

    /// A route catalog at the resident-menu boundary still uploads: the flat store's much larger
    /// catalog is the admission authority.
    @MainActor
    func testWholeTripUploadPastResidentMenuBoundary() {
        let app = launch(extraArgs: ["-OBCDeviceRoutesFull"])
        openTrip(app)
        app.buttons["trip.upload"].tap()

        XCTAssertTrue(
            app.staticTexts["upload.doneTitle"].waitForExistence(timeout: 20),
            "trip upload was incorrectly rejected at the menu boundary")
        snap(app, "TR8-trip-upload-menu-boundary")
        app.buttons["upload.done"].tap()
    }

    // MARK: Delete trip while connected

    /// Upload the trip, then delete it: the trip card is gone, and a route survives.
    @MainActor
    func testDeleteTripAfterUpload() {
        let app = launch()
        openTrip(app)

        // Land it on the device first.
        app.buttons["trip.upload"].tap()
        XCTAssertTrue(
            app.staticTexts["upload.doneTitle"].waitForExistence(timeout: 20),
            "trip upload never completed")
        app.buttons["upload.done"].tap()
        XCTAssertTrue(app.descendants(matching: .any)[dayAID].waitForExistence(timeout: 10), "did not return to trip page")

        // Delete the trip.
        app.buttons["trip.overflow"].tap()
        let delete = app.buttons["trip.delete"]
        XCTAssertTrue(delete.waitForExistence(timeout: 5), "overflow menu did not open")
        delete.tap()
        app.sheets.buttons["Delete trip"].tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons[tripCardID].waitForExistence(timeout: 3), "trip card survived delete")
        XCTAssertTrue(app.staticTexts["Kettle Moraine Loop"].exists, "route wrongly removed")
    }

}
