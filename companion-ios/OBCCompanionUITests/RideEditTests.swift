import XCTest

/// Ride editing through the real UI on the trips fixture, whose Day 2 has two rides that meet at
/// the Furka: the merge suggestion, edit mode with the shared handles, trim, revert and merge.
/// The edit arithmetic lives in `RideEditTests` and `RideEditModelTests`; this proves the wiring.
final class RideEditTests: XCTestCase {
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

    /// A listed ride card whose label names `name`.
    @MainActor
    private func card(_ app: XCUIApplication, _ name: String) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "label CONTAINS %@", name)).firstMatch
    }

    /// Drag a handle on the profile sideways by `dx` points.
    @MainActor
    private func drag(_ handle: XCUIElement, by dx: CGFloat) {
        XCTAssertTrue(handle.waitForExistence(timeout: 5), "\(handle) missing")
        let from = handle.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        from.press(forDuration: 0.2, thenDragTo: from.withOffset(CGVector(dx: dx, dy: 0)))
    }

    @MainActor
    private func openDay2(_ app: XCUIApplication) {
        let card = app.buttons["main.card.ride-day-2-ulrichen"]
        XCTAssertTrue(card.waitForExistence(timeout: 10), "Day 2 Ulrichen is not listed")
        card.tap()
        XCTAssertTrue(app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5))
    }

    @MainActor
    func testTrimRevertAndMergeADayWithTwoRides() {
        let app = XCUIApplication()
        app.launchArguments += [
            "-OBCScenario", "syncUpToDate", "-OBCFixtures", "trips",
            "-OBCHideMockHUD", "-OBCDisableAnimations",
            "-AppleLanguages", "(en)", "-AppleLocale", "en_US",
        ]
        app.launch()
        // The fixture rides are already synced, so Tracked lists them at launch.
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
        app.buttons["Rides"].tap()
        openDay2(app)

        let suggestion = app.otherElements["detail.mergeSuggestion"].buttons["quietRow.open"]
        XCTAssertTrue(suggestion.waitForExistence(timeout: 5), "the two Day 2 rides meet: a merge is suggested")
        snap(app, "detail-merge-suggestion")

        // Edit mode: trim both ends.
        app.buttons["detail.overflow"].tap()
        app.buttons["detail.editRide"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["rideEdit.screen"].firstMatch.waitForExistence(timeout: 5))
        drag(app.otherElements["Trim start"].firstMatch, by: 40)
        drag(app.otherElements["Trim end"].firstMatch, by: -30)
        XCTAssertTrue(app.staticTexts["rideEdit.summary"].label.hasPrefix("Keeps "))
        snap(app, "edit-trim")
        app.buttons["rideEdit.merge"].tap()
        XCTAssertTrue(app.buttons["rideEdit.mergeConfirm"].waitForExistence(timeout: 5), "the toolbar expands")
        snap(app, "edit-merge-expanded")
        app.buttons["rideEdit.mergeCancel"].tap()
        app.buttons["rideEdit.save"].tap()

        let title = app.staticTexts["detail.title"]
        XCTAssertTrue(title.waitForExistence(timeout: 5))
        XCTAssertEqual(title.label, "Day 2 Ulrichen", "a trim keeps the ride")
        snap(app, "detail-trimmed")

        // Revert restores the synced ride.
        app.buttons["detail.overflow"].tap()
        app.buttons["detail.revertRide"].tap()
        app.buttons["Revert"].tap()

        // Merge from the suggestion: the second ride joins this one.
        XCTAssertTrue(suggestion.waitForExistence(timeout: 5))
        suggestion.tap()
        let merge = app.buttons["detail.mergeSuggestion.merge"]
        XCTAssertTrue(merge.waitForExistence(timeout: 5), "the row expands and asks first")
        snap(app, "detail-merge-expanded")
        merge.tap()
        XCTAssertTrue(app.otherElements["summary.Distance"].waitForExistence(timeout: 5))
        XCTAssertFalse(suggestion.exists, "the merged ride has no next ride to merge")
        snap(app, "detail-merged")
        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(app.buttons["main.card.ride-day-2-ulrichen"].waitForExistence(timeout: 5))
        XCTAssertFalse(card(app, "Day 2 Ulrichen (2)").exists, "the second ride is part of the first")
    }
}
