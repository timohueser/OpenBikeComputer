import XCTest

/// The five real app screens embedded in the landing page's two bookend chapters.
///
/// Keep this suite deliberately small and deterministic: the capture script exports only
/// attachments whose names start with `website-`, fixes the simulator status bar, pins the locale,
/// and forces the offline map fallback. That makes the committed web assets reviewable and lets CI
/// catch a companion UI change that was not recaptured.
final class WebsiteScreenshotTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    private func launch(extraArguments: [String] = []) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += [
            "-OBCScenario", "happyPath",
            "-OBCNetwork", "offline",
            "-OBCHideMockHUD",
            "-AppleLanguages", "(en)",
            "-AppleLocale", "en_US",
        ]
        app.launchArguments += extraArguments
        app.launch()
        return app
    }

    @MainActor
    private func capture(_ app: XCUIApplication, name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = "website-\(name)"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    /// A real GPX import, followed through the real mock transport to the stable completion sheet.
    @MainActor
    func testRouteImportAndUploadBookend() {
        let app = launch(extraArguments: [
            "-OBCFixtures", "empty",
            "-OBCImportSample", "grimsel",
        ])

        XCTAssertTrue(
            app.staticTexts["Grimsel Pass"].waitForExistence(timeout: 10),
            "the imported route landing did not appear"
        )
        XCTAssertTrue(app.buttons["detail.upload"].waitForExistence(timeout: 5))
        capture(app, name: "route-imported")

        app.buttons["detail.upload"].tap()
        let begin = app.buttons["upload.begin"]
        XCTAssertTrue(begin.waitForExistence(timeout: 5), "the upload confirmation did not appear")
        begin.tap()

        XCTAssertTrue(
            app.staticTexts["On the device"].waitForExistence(timeout: 20),
            "the route upload did not finish"
        )
        capture(app, name: "route-on-device")
    }

    /// Pull the fixture rides off the mock device, then open one through the ordinary tracked list.
    @MainActor
    func testRideSyncAndDetailBookend() {
        let app = launch(extraArguments: ["-OBCFixtures", "website"])
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        app.buttons["Tracked"].tap()

        XCTAssertTrue(
            app.staticTexts["No rides yet"].waitForExistence(timeout: 5),
            "the phone should be waiting for the device ride before Finish"
        )
        capture(app, name: "rides-before-sync")

        let sync = app.buttons["topbar.sync"]
        XCTAssertTrue(sync.waitForExistence(timeout: 5))
        sync.tap()

        let line = app.descendants(matching: .any)["main.syncLine"].firstMatch
        let confirmed = NSPredicate(format: "label CONTAINS 'Synced 1 new ride just now'")
        let finished = expectation(for: confirmed, evaluatedWith: line)
        wait(for: [finished], timeout: 30)
        capture(app, name: "rides-synced")

        let card = app.buttons["main.card.ride-grimsel-pass"]
        XCTAssertTrue(card.waitForExistence(timeout: 5), "the downloaded ride was not listed")
        card.tap()
        XCTAssertTrue(
            app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5),
            "the downloaded ride detail did not open"
        )
        XCTAssertTrue(app.staticTexts["4.9 km"].waitForExistence(timeout: 5))
        XCTAssertTrue(
            app.descendants(matching: .any)["trackPreview.grid"].firstMatch.waitForExistence(timeout: 5),
            "the real Grimsel geometry should be visible in the ride hero"
        )
        capture(app, name: "ride-detail")
    }
}
