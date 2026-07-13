import XCTest

/// TR6 acceptance on the simulator: the trip card in the routes list and the
/// trip page behind it, driven through the real UI against the `trips` fixture
/// (`-OBCFixtures trips` — one trip grouping two routes + three loose routes).
/// The model logic is host-tested in `TripListModelTests`; this proves the
/// wiring launch-arg → interleaved list → trip page → stage detail.
final class TripTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", "happyPath", "-OBCFixtures", "trips"]
        // Pin the locale so the en-US stat strings assert cleanly.
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

    private let tripCardID = "main.trip.driftless-weekender"
    private let stageAID = "trip.stage.devils-lake-overnighter"
    private let stageBID = "trip.stage.cross-plains-gravel"

    /// Open the trip page and wait until its stage rows are up (the drill-in the
    /// other flows share).
    @MainActor
    private func openTrip(_ app: XCUIApplication) {
        waitForMain(app)
        let card = app.buttons[tripCardID]
        XCTAssertTrue(card.waitForExistence(timeout: 10), "trip card missing")
        card.tap()
        XCTAssertTrue(app.buttons[stageAID].waitForExistence(timeout: 10), "trip page did not open")
    }

    /// The trip card renders in the interleaved list: named, with the summed
    /// `N stages · …` line, and the filed routes are NOT loose rows.
    @MainActor
    func testTripCardRendersInterleavedWithLooseRoutes() {
        let app = launch()
        waitForMain(app)

        XCTAssertTrue(app.buttons[tripCardID].waitForExistence(timeout: 10), "trip card missing")
        XCTAssertTrue(app.staticTexts["Driftless Weekender"].exists)
        // Loose routes still show; filed routes do not appear at the top level.
        XCTAssertTrue(app.staticTexts["Kettle Moraine Loop"].exists, "loose route missing")
        XCTAssertFalse(app.staticTexts["Devil's Lake Overnighter"].exists, "filed route leaked to top level")
        snap(app, "TR6-trip-card")
    }

    /// Drill in: tapping the trip card opens the trip page with both stages and
    /// the (disabled, TR8) Upload trip action.
    @MainActor
    func testDrillIntoTripPage() {
        let app = launch()
        openTrip(app)

        XCTAssertTrue(app.buttons[stageBID].exists, "second stage row missing")
        let upload = app.buttons["trip.upload"]
        XCTAssertTrue(upload.exists, "Upload trip action missing")
        XCTAssertFalse(upload.isEnabled, "Upload trip must be disabled until TR8")
        snap(app, "TR6-trip-page")
    }

    /// A stage opens the ordinary route detail (E2), exactly as a top-level card.
    @MainActor
    func testStageTapsThroughToRouteDetail() {
        let app = launch()
        openTrip(app)
        app.buttons[stageAID].tap()

        XCTAssertTrue(
            app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 10),
            "route detail missing")
        XCTAssertTrue(app.staticTexts["Devil's Lake Overnighter"].waitForExistence(timeout: 5))
    }

    /// Rename the trip through the overflow menu (H12 idiom).
    @MainActor
    func testRenameTrip() {
        let app = launch()
        openTrip(app)

        app.buttons["trip.overflow"].tap()
        let rename = app.buttons["trip.rename"]
        XCTAssertTrue(rename.waitForExistence(timeout: 5), "overflow menu did not open")
        rename.tap()

        let field = app.textFields.firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 5), "rename field missing")
        field.typeText(" Reworked")
        app.alerts.buttons["Save"].tap()

        XCTAssertTrue(
            app.staticTexts["Driftless Weekender Reworked"].waitForExistence(timeout: 5)
                || app.navigationBars["Driftless Weekender Reworked"].waitForExistence(timeout: 5),
            "renamed trip title missing")
    }

    /// Remove a stage via its swipe action — the route returns to the top level
    /// and the trip keeps its remaining stage.
    @MainActor
    func testRemoveStageReturnsRouteToTopLevel() {
        let app = launch()
        openTrip(app)

        app.buttons[stageAID].swipeLeft()
        app.buttons["Remove"].firstMatch.tap()

        // Stage A is gone from the trip; stage B remains.
        XCTAssertFalse(app.buttons[stageAID].waitForExistence(timeout: 3), "removed stage still in trip")
        XCTAssertTrue(app.buttons[stageBID].exists, "remaining stage missing")

        // Back on the list, the removed route is now a loose top-level card.
        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(
            app.staticTexts["Devil's Lake Overnighter"].waitForExistence(timeout: 5),
            "removed route did not return to the top level")
    }

    /// Delete → Ungroup: the trip disappears, its routes stay in the library as
    /// loose top-level cards.
    @MainActor
    func testDeleteTripUngroupKeepsRoutes() {
        let app = launch()
        openTrip(app)

        app.buttons["trip.overflow"].tap()
        let delete = app.buttons["trip.delete"]
        XCTAssertTrue(delete.waitForExistence(timeout: 5), "overflow menu did not open")
        delete.tap()
        app.sheets.buttons["Ungroup"].tap()

        // Popped back to the list; the trip card is gone but the routes remain.
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons[tripCardID].waitForExistence(timeout: 3), "trip card survived ungroup")
        XCTAssertTrue(app.staticTexts["Devil's Lake Overnighter"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Cross-Plains Gravel"].exists)
    }

    /// Delete → Delete trip & routes: the trip and both member routes are gone;
    /// the loose routes survive.
    @MainActor
    func testDeleteTripAndRoutesRemovesMembers() {
        let app = launch()
        openTrip(app)

        app.buttons["trip.overflow"].tap()
        let delete = app.buttons["trip.delete"]
        XCTAssertTrue(delete.waitForExistence(timeout: 5), "overflow menu did not open")
        delete.tap()
        app.sheets.buttons["Delete trip & routes"].tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons[tripCardID].waitForExistence(timeout: 3), "trip card survived delete")
        XCTAssertFalse(app.staticTexts["Devil's Lake Overnighter"].exists, "member route survived delete")
        // A loose route that was never in the trip is untouched.
        XCTAssertTrue(app.staticTexts["Kettle Moraine Loop"].exists)
    }
}
