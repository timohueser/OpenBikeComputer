import XCTest

final class ReplayTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor private func launch(extraArguments: [String] = []) -> XCUIApplication {
        XCUIDevice.shared.orientation = .portrait
        let app = XCUIApplication()
        app.launchArguments = ["-OBCScenario", "syncUpToDate", "-OBCFixtures", "journal",
                               "-OBCHideMockHUD", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launchArguments += extraArguments
        app.launch()
        XCTAssertTrue(app.buttons["main.trip.alps-traverse"].waitForExistence(timeout: 10))
        app.buttons["main.trip.alps-traverse"].tap()
        let entry = app.buttons["trip.journal.replay"]
        XCTAssertTrue(entry.waitForExistence(timeout: 10))
        if !entry.isHittable { app.swipeUp() }
        entry.tap()
        XCTAssertTrue(app.buttons["replay.close"].waitForExistence(timeout: 10))
        return app
    }

    @MainActor func testReplayOpensFromRiddenDaysAndClosesWhileLoading() {
        let app = launch()
        XCTAssertTrue(app.descendants(matching: .any)["replay.profile"].firstMatch.exists)
        XCTAssertEqual(app.sliders.count, 0, "The profile is the only scrub control")
        app.buttons["replay.close"].tap()
        XCTAssertTrue(app.buttons["trip.journal.replay"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["replay.close"].exists)
    }

    @MainActor func testLargeTextAndLandscapeKeepControlsReachable() {
        let app = launch(extraArguments: ["-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL",
                                          "-obc.appearance", "dark"])
        let live = ProcessInfo.processInfo.environment["OBC_REPLAY_LIVE"] == "1"
        if live {
            let ready = NSPredicate(format: "enabled == true")
            XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: ready, object: app.buttons["replay.play"])], timeout: 60), .completed)
        }
        XCTAssertTrue(app.buttons["replay.close"].isHittable)
        capture(app, "replay-dark-large-text")
        XCUIDevice.shared.orientation = .landscapeLeft
        let rotated = NSPredicate { _, _ in app.frame.width > app.frame.height }
        XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: rotated, object: app)], timeout: 5), .completed)
        defer { XCUIDevice.shared.orientation = .portrait }
        XCTAssertTrue(app.buttons["replay.close"].isHittable)
        let dock = app.scrollViews["replay.controls"]
        if !app.buttons["replay.play"].isHittable { dock.swipeUp() }
        XCTAssertTrue(app.buttons["replay.play"].isHittable)
        if live {
            let map = app.webViews.firstMatch
            let from = map.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
            from.press(forDuration: 0.2, thenDragTo: from.withOffset(CGVector(dx: 45, dy: 0)))
            XCTAssertTrue(app.buttons["Reset camera"].waitForExistence(timeout: 5))
        }
        capture(app, "replay-dark-large-landscape")
        app.buttons["replay.close"].tap()
    }

    /// Provider-dependent device evidence is opt-in; ordinary CI does not depend on public tiles.
    @MainActor func testLiveCameraProfileAndLifecycle() throws {
        try XCTSkipUnless(ProcessInfo.processInfo.environment["OBC_REPLAY_LIVE"] == "1")
        let app = launch()
        let play = app.buttons["replay.play"]
        let ready = NSPredicate(format: "enabled == true")
        XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: ready, object: play)], timeout: 60), .completed)
        capture(app, "replay-portrait")
        play.tap()
        let map = app.webViews.firstMatch
        XCTAssertTrue(map.exists)
        let from = map.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        from.press(forDuration: 0.2, thenDragTo: from.withOffset(CGVector(dx: 70, dy: 30)))
        XCTAssertTrue(app.buttons["Reset camera"].waitForExistence(timeout: 5))
        app.buttons["replay.overview"].tap()
        XCTAssertTrue(app.buttons["Follow rider"].waitForExistence(timeout: 5))
        capture(app, "replay-overview")
        app.buttons["replay.overview"].tap()
        XCTAssertTrue(app.buttons["Reset camera"].waitForExistence(timeout: 5))
        let profile = app.descendants(matching: .any)["replay.profile"].firstMatch
        let start = profile.coordinate(withNormalizedOffset: CGVector(dx: 0.2, dy: 0.5))
        start.press(forDuration: 0.2, thenDragTo: profile.coordinate(withNormalizedOffset: CGVector(dx: 0.65, dy: 0.5)))
        XCTAssertEqual(play.label, "Play")
        capture(app, "replay-adjusted-scrub")
        XCUIDevice.shared.orientation = .landscapeLeft
        let rotated = NSPredicate { _, _ in app.frame.width > app.frame.height }
        XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: rotated, object: app)], timeout: 5), .completed)
        capture(app, "replay-landscape")
        XCUIDevice.shared.orientation = .portrait
        XCUIDevice.shared.press(.home)
        app.activate()
        XCTAssertEqual(XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: ready, object: play)], timeout: 60), .completed)
        XCTAssertEqual(play.label, "Play")
        XCTAssertTrue(app.buttons["Reset camera"].waitForExistence(timeout: 5))
        app.buttons["replay.close"].tap()
        XCTAssertTrue(app.buttons["trip.journal.replay"].waitForExistence(timeout: 5))
    }

    @MainActor private func capture(_ app: XCUIApplication, _ name: String) {
        let image = XCTAttachment(screenshot: app.screenshot())
        image.name = name
        image.lifetime = .keepAlways
        add(image)
    }
}
