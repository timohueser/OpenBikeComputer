import XCTest

/// The main screen's design states driven through the real UI against the
/// mock. The same logic is host-tested in `MainScreenModelTests`; this
/// proves the wiring launch-arg → scenario → screen.
final class MainScreenTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch(scenario: String, fixtures: String? = nil) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", scenario]
        if let fixtures { app.launchArguments += ["-OBCFixtures", fixtures] }
        // Pin the locale: `OBCFormat` localizes numbers ("62,4 km" on a German
        // sim), and these tests assert the design's en-US strings.
        app.launchArguments += ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        return app
    }

    /// Keep a named screenshot in the result bundle — the visual record of each
    /// design screen (export with `xcresulttool export attachments`).
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

    /// Compact rows with the per-tab stat lines.
    @MainActor
    func testPlannedAndTrackedTabsShowCompactRows() {
        let app = launch(scenario: "happyPath")
        waitForMain(app)

        XCTAssertTrue(app.staticTexts["Kettle Moraine Loop"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["62.4 km · 840 m ↑ · 3h 20m"].exists, "C1 stat line wrong")
        snap(app, "C1-main-planned")

        app.buttons["Tracked"].tap()
        // Tracked is library-first: rides show only after a sync pulls them
        // in — an un-synced device ride is never a half-empty row.
        app.buttons["topbar.sync"].tap()
        XCTAssertTrue(app.staticTexts["Sunday Coffee Spin"].waitForExistence(timeout: 30))
        let statLine = app.staticTexts.matching(
            NSPredicate(format: "label CONTAINS '31.6 km' AND label CONTAINS 'kph'")
        ).firstMatch
        XCTAssertTrue(statLine.waitForExistence(timeout: 5),
                      "C2 tracked stat line (date · distance · time · avg) missing")
        snap(app, "C2-main-tracked")

        app.buttons["Planned"].tap()
        XCTAssertTrue(app.staticTexts["Kettle Moraine Loop"].waitForExistence(timeout: 5))
    }

    /// Search hides until a pull-down reveals it (Mail-style); then it
    /// filters, and no matches shows an empty-state with the query kept editable.
    @MainActor
    func testSearchRevealsOnPullThenFiltersAndShowsH6() {
        let app = launch(scenario: "happyPath")
        waitForMain(app)
        XCTAssertTrue(app.staticTexts["Kettle Moraine Loop"].waitForExistence(timeout: 10))

        let search = app.textFields.firstMatch
        XCTAssertFalse(search.exists, "search must stay hidden until pulled")
        snap(app, "C1-search-hidden")

        app.swipeDown()   // over-scroll the list → the bar slides in
        XCTAssertTrue(search.waitForExistence(timeout: 5), "pull did not reveal search")
        search.tap()
        search.typeText("sugar")
        XCTAssertTrue(app.staticTexts["Sugar River Trail"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.staticTexts["Kettle Moraine Loop"].exists, "filter did not apply")

        search.typeText(" trail zz")
        let noMatches = app.staticTexts["main.noMatches"]
        XCTAssertTrue(noMatches.waitForExistence(timeout: 5), "H6 missing")
        snap(app, "H6-search-no-matches")

        app.buttons["Clear search"].tap()
        XCTAssertTrue(app.staticTexts["Kettle Moraine Loop"].waitForExistence(timeout: 5))
    }

    /// The bar is transient (Mail-style): once the cleared bar scrolls off the
    /// top it un-reveals — back at the top the list is search-free again.
    @MainActor
    func testSearchHidesAgainAfterScrollingAway() {
        let app = launch(scenario: "happyPath", fixtures: "large")
        waitForMain(app)
        XCTAssertTrue(app.staticTexts["Kettle Moraine Loop"].waitForExistence(timeout: 10))

        app.swipeDown()
        let search = app.textFields.firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: 5), "pull did not reveal search")

        app.swipeUp(velocity: .fast)   // scroll the (empty) bar off the top
        // Return toward the top *without* momentum — a flick would over-scroll
        // and legitimately re-reveal the bar. Held drags don't bounce.
        let from = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.3))
        let to = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.85))
        for _ in 0..<8 where !app.buttons["Planned"].isHittable {
            from.press(forDuration: 0.05, thenDragTo: to, withVelocity: 400, thenHoldForDuration: 0.3)
        }
        // The selector row is on screen, so a still-revealed bar (right below
        // it) would be too — its absence means it re-hid.
        XCTAssertTrue(app.buttons["Planned"].isHittable, "did not make it back to the top")
        XCTAssertFalse(search.exists, "search must re-hide once scrolled away")
        snap(app, "C1-search-rehidden")
    }

    /// Swipe reveals Delete; the tap deletes directly — the swipe reveal is
    /// the deliberate second action, no extra confirm.
    @MainActor
    func testSwipeToDeleteRemovesTheRowDirectly() {
        let app = launch(scenario: "happyPath")
        waitForMain(app)

        let card = app.buttons["main.card.kettle-moraine-loop"]
        XCTAssertTrue(card.waitForExistence(timeout: 10))
        card.swipeLeft()

        let reveal = app.buttons["Delete"]
        XCTAssertTrue(reveal.waitForExistence(timeout: 5), "H11 swipe action missing")
        snap(app, "H11-swipe-to-delete")
        reveal.tap()

        let gone = NSPredicate(format: "exists == false")
        let expectation = expectation(for: gone, evaluatedWith: card)
        wait(for: [expectation], timeout: 5)
        XCTAssertTrue(app.staticTexts["Sugar River Trail"].exists, "other rows must survive")
    }

    /// A deleted ride must stay deleted: the device still lists it (its copy
    /// stays on the SD card), but a later sync must neither re-download nor
    /// re-list it. Tracked is library-first, so the ride is synced in first,
    /// then deleted, then a re-sync must leave it gone.
    @MainActor
    func testDeletedRideDoesNotResurrectOnSync() {
        let app = launch(scenario: "happyPath")
        waitForMain(app)
        app.buttons["Tracked"].tap()

        // Sync pulls the rides in (library-first).
        app.buttons["topbar.sync"].tap()
        let card = app.buttons["main.card.ride-sunday-coffee-spin"]
        XCTAssertTrue(card.waitForExistence(timeout: 30))

        card.swipeLeft()
        let reveal = app.buttons["Delete"]
        XCTAssertTrue(reveal.waitForExistence(timeout: 5))
        reveal.tap()

        let gone = NSPredicate(format: "exists == false")
        wait(for: [expectation(for: gone, evaluatedWith: card)], timeout: 5)

        // A second sync: nothing new (the deleted ride stays tombstoned, its
        // SD-card copy untouched) — it must not come back.
        app.buttons["topbar.sync"].tap()
        let toast = app.staticTexts.matching(
            NSPredicate(format: "label CONTAINS 'up to date'")
        ).firstMatch
        XCTAssertTrue(toast.waitForExistence(timeout: 15), "expected the H9 up-to-date toast")
        XCTAssertFalse(card.exists, "deleted ride resurrected by sync")
        snap(app, "SYNC-after-delete-no-resurrect")
    }

    /// SYNC states: idle → syncing → done + "Synced N new rides just now";
    /// a second sync is the quiet up-to-date toast.
    @MainActor
    func testSyncCyclesAndConfirmsThenReportsUpToDate() {
        let app = launch(scenario: "happyPath")
        waitForMain(app)
        app.buttons["Tracked"].tap()
        // Library-first: no rows until the first sync — that first sync is
        // exactly what this test drives.
        let sync = app.buttons["topbar.sync"]
        XCTAssertTrue(sync.isEnabled)
        sync.tap()

        // The confirm line lands when the batch completes (~8 s of mock
        // throughput); the ~2 s forest check on the button rides along.
        let line = app.descendants(matching: .any)["main.syncLine"].firstMatch
        XCTAssertTrue(line.waitForExistence(timeout: 5), "sync progress line missing")
        let confirmed = NSPredicate(format: "label CONTAINS 'Synced 4 new rides just now'")
        let expectation = expectation(for: confirmed, evaluatedWith: line)
        wait(for: [expectation], timeout: 30)
        snap(app, "SYNC-done-confirm-line")

        sync.tap()
        let toast = app.staticTexts.matching(
            NSPredicate(format: "label CONTAINS 'up to date'")
        ).firstMatch
        XCTAssertTrue(toast.waitForExistence(timeout: 10), "H9 toast missing")
        snap(app, "H9-up-to-date")
    }

    /// Out of range degrades — banner + dimmed sync, library browsable.
    @MainActor
    func testOutOfRangeShowsBannerAndDisablesSync() {
        let app = launch(scenario: "outOfRange")
        waitForMain(app)

        XCTAssertTrue(app.otherElements["disconnectedBanner"].firstMatch.waitForExistence(timeout: 10)
                      || app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'out of range'")).firstMatch.exists,
                      "S4 banner missing")
        XCTAssertTrue(app.staticTexts["Kettle Moraine Loop"].waitForExistence(timeout: 10),
                      "library must stay browsable")
        XCTAssertFalse(app.buttons["topbar.sync"].isEnabled, "sync must dim when unreachable")
        snap(app, "S4-out-of-range")
    }

    /// Card tap → the route detail (walked in depth by `RouteDetailTests`).
    @MainActor
    func testCardTapPushesDetail() {
        let app = launch(scenario: "happyPath")
        waitForMain(app)

        let card = app.buttons["main.card.kettle-moraine-loop"]
        XCTAssertTrue(card.waitForExistence(timeout: 10))
        card.tap()
        XCTAssertTrue(app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5), "detail missing")
    }

    /// The + opens the Files picker directly — no intermediate menu with
    /// dead rows.
    @MainActor
    func testImportButtonOpensFilePickerDirectly() {
        let app = launch(scenario: "happyPath")
        waitForMain(app)

        app.buttons["Import a route"].tap()
        // The system document picker; Cancel is its stable anchor.
        let cancel = app.buttons["Cancel"]
        XCTAssertTrue(cancel.waitForExistence(timeout: 10), "Files picker did not open")
        snap(app, "I2-files-picker")
        cancel.tap()
        waitForMain(app)
    }

    /// An empty library points at import, it doesn't dead-end.
    @MainActor
    func testEmptyLibraryShowsS1() {
        let app = launch(scenario: "emptyLibrary")
        waitForMain(app)
        XCTAssertTrue(app.staticTexts["No planned routes yet"].waitForExistence(timeout: 10), "S1 missing")
        snap(app, "S1-empty-library")
    }
}
