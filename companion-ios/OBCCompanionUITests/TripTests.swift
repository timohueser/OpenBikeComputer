import XCTest

/// The trip card in the routes list and the trip page behind it, driven through the real UI
/// against the trips fixture: one two-day trip, plus three routes. The model logic is host-tested
/// in `TripListModelTests`; this proves the wiring from launch argument to interleaved list to
/// trip page.
final class TripTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", "happyPath", "-OBCFixtures", "trips"]
        // Pin the locale so the stat strings assert cleanly.
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
    /// Open the trip page and wait until its day rows are up: the drill-in the other flows share.
    @MainActor
    private func openTrip(_ app: XCUIApplication) {
        waitForMain(app)
        let card = app.buttons[tripCardID]
        XCTAssertTrue(card.waitForExistence(timeout: 10), "trip card missing")
        card.tap()
        XCTAssertTrue(day(app, 0).waitForExistence(timeout: 10), "trip page did not open")
    }

    @MainActor
    private func day(_ app: XCUIApplication, _ index: Int) -> XCUIElement {
        app.descendants(matching: .any)["trip.day.\(index)"].firstMatch
    }

    /// The trip card renders in the interleaved list, named and with the summed day line, and
    /// the joined routes are not route rows.
    @MainActor
    func testTripCardRendersInterleavedWithLooseRoutes() {
        let app = launch()
        waitForMain(app)

        XCTAssertTrue(app.buttons[tripCardID].waitForExistence(timeout: 10), "trip card missing")
        XCTAssertTrue(app.staticTexts["Driftless Weekender"].exists)
        // Routes still show; the joined routes live only in the trip's line.
        XCTAssertTrue(app.staticTexts["Kettle Moraine Loop"].exists, "route missing")
        XCTAssertFalse(app.staticTexts["Devil's Lake Overnighter"].exists, "joined route leaked to top level")
        snap(app, "TR6-trip-card")
    }

    /// Tapping the trip card opens the trip page with both days, the transfer line between them, and
    /// the Upload trip action, enabled because the connected device holds no copy of this trip yet.
    @MainActor
    func testDrillIntoTripPage() {
        let app = launch()
        openTrip(app)

        XCTAssertTrue(day(app, 1).exists, "second day row missing")
        XCTAssertTrue(
            app.descendants(matching: .any)["trip.transfer.0"].exists, "the two days have a transfer between them")
        let upload = app.buttons["trip.upload"]
        XCTAssertTrue(upload.exists, "Upload trip action missing")
        XCTAssertTrue(upload.isEnabled, "Upload trip must be enabled on a connected device (TR8)")
        snap(app, "TR6-trip-page")
    }

    /// A tap on the transfer line labels it; None takes the label off again.
    @MainActor
    func testLabelATransfer() {
        let app = launch()
        openTrip(app)

        let line = app.descendants(matching: .any)["trip.transfer.0"].firstMatch
        XCTAssertTrue(line.label.hasPrefix("Transfer · "), "an unlabelled transfer: \(line.label)")
        line.tap()
        app.buttons["Train"].firstMatch.tap()
        XCTAssertTrue(
            app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH 'Train · '")).firstMatch
                .waitForExistence(timeout: 5), "the line reads Train")
        snap(app, "trip-transfer-train")

        app.descendants(matching: .any)["trip.transfer.0"].firstMatch.tap()
        app.buttons["None"].firstMatch.tap()
        XCTAssertTrue(
            app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH 'Transfer · '")).firstMatch
                .waitForExistence(timeout: 5), "the label is off again")
    }

    /// Rename the trip through the overflow menu.
    @MainActor
    func testRenameTrip() {
        let app = launch()
        openTrip(app)

        app.buttons["trip.overflow"].tap()
        let rename = app.buttons["trip.rename"]
        XCTAssertTrue(rename.waitForExistence(timeout: 5), "overflow menu did not open")
        rename.tap()

        let field = app.textFields["rename.field"]
        XCTAssertTrue(field.waitForExistence(timeout: 5), "rename field missing")
        field.typeText(" Reworked")
        app.buttons["rename.save"].tap()

        XCTAssertTrue(
            app.staticTexts["Driftless Weekender Reworked"].waitForExistence(timeout: 5)
                || app.navigationBars["Driftless Weekender Reworked"].waitForExistence(timeout: 5),
            "renamed trip title missing")
    }

    /// Delete the trip: the trip card is gone, and the routes survive.
    @MainActor
    func testDeleteTrip() {
        let app = launch()
        openTrip(app)

        app.buttons["trip.overflow"].tap()
        let delete = app.buttons["trip.delete"]
        XCTAssertTrue(delete.waitForExistence(timeout: 5), "overflow menu did not open")
        delete.tap()
        app.sheets.buttons["Delete trip"].tap()

        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons[tripCardID].waitForExistence(timeout: 3), "trip card survived delete")
        XCTAssertTrue(app.staticTexts["Kettle Moraine Loop"].exists)
    }

    /// Day 1 ends at a transfer: Day 2 starts 35 km away. The stops sheet lists the fixture
    /// campground, says why the day end stays, and does not offer the pick.
    @MainActor
    func testStopsAtATransferAreShownButNotPicked() {
        let app = launch()
        openTrip(app)

        day(app, 0).press(forDuration: 1)
        let stops = app.buttons["trip.day.stops"]
        XCTAssertTrue(stops.waitForExistence(timeout: 5), "day menu did not open")
        stops.tap()
        let camp = app.buttons["stops.row"].firstMatch
        XCTAssertTrue(camp.waitForExistence(timeout: 10), "no stop in the sheet")
        XCTAssertTrue(app.staticTexts["Devil's Lake State Park Campgrounds"].exists)
        XCTAssertTrue(app.staticTexts["This day ends at a transfer."].exists)
        XCTAssertFalse(camp.isEnabled, "a pick would put the gap inside a day")
    }
}
