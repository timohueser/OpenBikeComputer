import XCTest

/// The sync edge cases the scenarios stage: a link that drops mid-sync, where the warning banner
/// keeps the partial and Resume finishes the batch, and an up-to-date first sync. The happy cycle
/// lives in `MainScreenTests`, and the model logic is host-tested in `MainScreenModelTests`.
final class SyncTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch(scenario: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", scenario]
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

    /// The scenario cuts the link partway through the batch. The banner keeps the partial count,
    /// Resume continues the same transfer, and the full batch confirms: nothing re-counted,
    /// nothing lost.
    @MainActor
    func testSyncDropShowsH10ThenResumeFinishesTheBatch() {
        let app = launch(scenario: "syncDrop")
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
        app.buttons["Rides"].tap()
        // Tracked is library-first, so there are no rows until a sync, and this sync drops
        // mid-batch. The banner, not the rows, is the subject.
        app.buttons["topbar.sync"].tap()

        // The title and message render as one combined text, and the landed count depends on ride
        // sizes, so match the shape.
        let banner = app.staticTexts.matching(
            NSPredicate(format: "label BEGINSWITH 'Sync interrupted.' AND label CONTAINS 'of 4 rides'")
        ).firstMatch
        XCTAssertTrue(banner.waitForExistence(timeout: 20), "H10 banner missing")
        XCTAssertFalse(app.buttons["topbar.sync"].isEnabled,
                       "the dropped link dims sync — Resume is the way forward")
        snap(app, "H10-sync-interrupted")

        app.buttons["Resume"].tap()

        // Resume restores the link and the rest of the batch lands; the confirm line counts every
        // ride of the batch exactly once.
        let line = app.descendants(matching: .any)["main.syncLine"].firstMatch
        let confirmed = NSPredicate(format: "label CONTAINS 'Synced 4 new rides just now'")
        let expectation = expectation(for: confirmed, evaluatedWith: line)
        wait(for: [expectation], timeout: 30)
        XCTAssertFalse(banner.exists, "the banner comes down once the sync completes")
        snap(app, "H10-after-resume-confirm")
    }

    /// The scenario seeds the library as fully synced, so the first sync is already up to date: a
    /// quiet toast, straight back to idle, and no empty done screen.
    @MainActor
    func testSyncUpToDateScenarioToastsOnFirstSync() {
        let app = launch(scenario: "syncUpToDate")
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
        app.buttons["Rides"].tap()
        XCTAssertTrue(app.staticTexts["Sunday Coffee Spin"].waitForExistence(timeout: 10))

        app.buttons["topbar.sync"].tap()
        let toast = app.staticTexts.matching(
            NSPredicate(format: "label CONTAINS 'up to date'")
        ).firstMatch
        XCTAssertTrue(toast.waitForExistence(timeout: 10), "H9 toast missing on first sync")
        snap(app, "H9-first-sync-up-to-date")

        // No progress line and no confirm line: nothing was transferred.
        XCTAssertFalse(app.descendants(matching: .any)["main.syncLine"].firstMatch.exists,
                       "an up-to-date sync must not show a sync line")
    }
}
