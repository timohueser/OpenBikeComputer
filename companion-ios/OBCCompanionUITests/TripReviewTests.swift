import XCTest

/// The trip page of a trip with rides is the trip review, driven through the real UI against the
/// journal fixture: a four-day trip with a train after Day 1, and Day 2 ridden in two rides that
/// stop before the planned day end. The review logic is host-tested in `TripReviewTests` and
/// `TripJournalModelTests`; this proves the wiring from the ride list to the journal.
final class TripReviewTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func snap(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    @MainActor
    private func element(_ app: XCUIApplication, _ id: String) -> XCUIElement {
        app.descendants(matching: .any)[id].firstMatch
    }

    @MainActor
    func testATripWithRidesReadsAsTheJournal() {
        let app = XCUIApplication()
        // The fixture rides are already synced, so the trip has them at launch.
        app.launchArguments += [
            "-OBCScenario", "syncUpToDate", "-OBCFixtures", "journal",
            "-OBCHideMockHUD", "-OBCDisableAnimations",
            "-AppleLanguages", "(en)", "-AppleLocale", "en_US",
        ]
        app.launch()
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
        let card = app.buttons["main.trip.alps-traverse"]
        XCTAssertTrue(card.waitForExistence(timeout: 10), "trip card missing")
        card.tap()

        XCTAssertTrue(element(app, "trip.journal.day.0").waitForExistence(timeout: 10), "the review did not open")
        XCTAssertTrue(element(app, "trip.journal.day.1").exists, "Day 2 is ridden")
        XCTAssertTrue(element(app, "trip.journal.transfer.0").exists, "the train after Day 1")
        XCTAssertTrue(
            app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH '57.5 of 89.8 km'")).firstMatch.exists,
            "the totals are the ridden km of the planned km")
        snap(app, "trip-review-top")

        // The days still to ride stay rows.
        let day3 = element(app, "trip.day.2")
        app.swipeUp()
        XCTAssertTrue(day3.waitForExistence(timeout: 5), "Day 3 is a row")
        XCTAssertFalse(element(app, "trip.day.0").exists, "a ridden day is not a row")
        snap(app, "trip-review-rest")

        // Day 2 has two rides; each opens its ride.
        app.swipeDown()
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Day 2 Ulrichen (2)'")).firstMatch.tap()
        XCTAssertTrue(element(app, "detail.screen").waitForExistence(timeout: 5), "the ride did not open")
    }
}
