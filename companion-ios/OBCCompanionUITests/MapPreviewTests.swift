import XCTest

/// The route detail hero offers the interactive map when online, and degrades to the grid preview,
/// with no expand affordance and no blank map, when forced offline. The decision itself is
/// host-tested in `MapPreviewModeTests`; this pins the wiring through the real UI. Network state is
/// pinned by a launch argument, never by real connectivity, so the fallback is deterministic.
final class MapPreviewTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch(network: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCScenario", "happyPath", "-OBCNetwork", network]
        app.launchArguments += ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        return app
    }

    @MainActor
    private func openPlannedDetail(_ app: XCUIApplication) {
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10), "main missing")
        let card = app.buttons["main.card.kettle-moraine-loop"]
        XCTAssertTrue(card.waitForExistence(timeout: 10))
        card.tap()
        XCTAssertTrue(
            app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5),
            "detail missing"
        )
    }

    /// Online, the hero is a button into the full-screen interactive map.
    @MainActor
    func testOnlineHeroOpensInteractiveMap() {
        let app = launch(network: "online")
        openPlannedDetail(app)

        let expand = app.buttons["detail.expandMap"]
        XCTAssertTrue(expand.waitForExistence(timeout: 5), "expand-map affordance missing while online")
        expand.tap()

        // The cover's Done toolbar button is the reliable "map opened" signal: the map view's own
        // accessibility element can lag its tiles.
        let done = app.buttons["Done"]
        XCTAssertTrue(done.waitForExistence(timeout: 10), "interactive map cover didn't open")
        done.tap()
        XCTAssertTrue(
            app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5),
            "Done didn't return to the detail"
        )
    }

    /// Offline, the grid fallback has no expand affordance and no map to open.
    @MainActor
    func testOfflineHeroFallsBackToGrid() {
        let app = launch(network: "offline")
        openPlannedDetail(app)

        // The detail is up, and the map affordance must be absent.
        XCTAssertFalse(
            app.buttons["detail.expandMap"].exists,
            "offline detail must not offer the interactive map"
        )
    }

    @MainActor
    func testOfflineSelectionFollowsTheMapAndOnlyCornerDragsResizeIt() {
        let app = XCUIApplication()
        app.launchArguments = ["-OBCScenario", "happyPath", "-OBCShowPlanner", "-OBCHideMockHUD",
                               "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        let layers = app.buttons["planner.layers"]
        XCTAssertTrue(layers.waitForExistence(timeout: 10))
        layers.tap()
        app.buttons["planner.downloadMap"].tap()

        let northwest = app.descendants(matching: .any)["offline.resize.northwest"].firstMatch
        let southeast = app.descendants(matching: .any)["offline.resize.southeast"].firstMatch
        XCTAssertTrue(northwest.waitForExistence(timeout: 10))
        XCTAssertTrue(southeast.exists)
        XCTAssertFalse(app.navigationBars.buttons["Offline maps"].exists)
        let initialNW = northwest.frame, initialSE = southeast.frame
        let map = app.descendants(matching: .any)["offline.areaMap"].firstMatch
        let centre = map.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        centre.press(forDuration: 0.1, thenDragTo: centre.withOffset(CGVector(dx: 25, dy: 20)))
        XCTAssertTrue(waitUntil { abs(northwest.frame.midX - initialNW.midX) > 10 })
        let movedNW = northwest.frame, movedSE = southeast.frame
        XCTAssertEqual(movedSE.midX - movedNW.midX, initialSE.midX - initialNW.midX, accuracy: 2)
        XCTAssertEqual(movedSE.midY - movedNW.midY, initialSE.midY - initialNW.midY, accuracy: 2)

        // A thumb can land outside the visible handle and still resize the selection.
        let corner = northwest.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
            .withOffset(CGVector(dx: 30, dy: 0))
        corner.press(forDuration: 0.1, thenDragTo: corner.withOffset(CGVector(dx: 25, dy: 25)))
        XCTAssertTrue(waitUntil { northwest.frame.midX > movedNW.midX + 10 })
        XCTAssertEqual(southeast.frame.midX, movedSE.midX, accuracy: 2)
        XCTAssertEqual(southeast.frame.midY, movedSE.midY, accuracy: 2)

        let width = southeast.frame.midX - northwest.frame.midX
        map.pinch(withScale: 1.2, velocity: 1)
        XCTAssertTrue(waitUntil { southeast.frame.midX - northwest.frame.midX > width + 10 })
        app.buttons["offline.fit"].tap()
        app.buttons["offline.done"].tap()
        XCTAssertTrue(layers.waitForExistence(timeout: 5))
        layers.tap()
        app.buttons["planner.downloadMap"].tap()
        XCTAssertTrue(northwest.waitForExistence(timeout: 5))
        app.buttons["offline.done"].tap()
        XCTAssertTrue(layers.waitForExistence(timeout: 5))
    }

    @MainActor
    private func waitUntil(_ predicate: @escaping @MainActor () -> Bool) -> Bool {
        let condition = NSPredicate { _, _ in MainActor.assumeIsolated { predicate() } }
        return XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: condition, object: nil)], timeout: 5) == .completed
    }
}
