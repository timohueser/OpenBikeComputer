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
}
