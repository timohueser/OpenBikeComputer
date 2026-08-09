import XCTest

/// The five real app screens embedded in the landing page's two bookend chapters.
///
/// Keep this suite deliberately small and deterministic: the capture script exports only
/// attachments whose names start with `website-`, fixes the simulator status bar, pins the locale,
/// and forces the offline map fallback. That makes the committed web assets reviewable and lets CI
/// catch a companion UI change that was not recaptured.
///
/// **Determinism is the whole job** (#1212). The drift gate compares pixels, so a capture must
/// happen only once the screen has finished becoming itself — and must never aim at a state that
/// expires on a clock. Four mechanisms:
///
///   1. the app runs with `-OBCDisableAnimations`, so no transition is ever in flight;
///   2. `-OBCHoldSyncConfirmation` parks the post-sync check the `rides-synced` shot is *of*,
///      instead of leaving it two seconds to be photographed in;
///   3. every page waits for the elements it draws **asynchronously** — a ride's elevation profile
///      arrives with the `rideDetail` read, not with the summary, so waiting on the stat labels
///      alone could screenshot a screen whose lower half had not been laid out yet;
///   4. `capture` then waits for two consecutive identical frames, because the render can still
///      trail the accessibility tree by a beat on a slow CI runner.
final class WebsiteScreenshotTests: XCTestCase {
    /// How many screenshots `capture` will take waiting for two identical ones. Each round trip is
    /// tens of milliseconds, so this is seconds of headroom on a loaded CI VM — and a bounded poll,
    /// never a blind sleep.
    private static let settleAttempts = 40

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
            "-OBCDisableAnimations",
            "-OBCHoldSyncConfirmation",
            "-AppleLanguages", "(en)",
            "-AppleLocale", "en_US",
        ]
        app.launchArguments += extraArguments
        app.launch()
        return app
    }

    /// Wait for an accessibility identifier anywhere in the tree (the screenshot surfaces are
    /// `Canvas`/container views as often as they are controls, so don't narrow by element type).
    @MainActor
    @discardableResult
    private func waitFor(
        _ app: XCUIApplication, _ identifier: String, timeout: TimeInterval = 10,
        _ message: String, file: StaticString = #filePath, line: UInt = #line
    ) -> XCUIElement {
        let element = app.descendants(matching: .any)[identifier].firstMatch
        XCTAssertTrue(element.waitForExistence(timeout: timeout), message, file: file, line: line)
        return element
    }

    /// Screenshot the app once it has stopped changing: keep capturing until two consecutive frames
    /// are pixel-identical, then attach that frame. Element existence proves the *data* landed;
    /// this proves the *frame* did.
    @MainActor
    private func capture(
        _ app: XCUIApplication, name: String, file: StaticString = #filePath, line: UInt = #line
    ) {
        var previous = app.screenshot()
        var settled: XCUIScreenshot?
        for _ in 0..<Self.settleAttempts {
            let next = app.screenshot()
            if next.pngRepresentation == previous.pngRepresentation {
                settled = next
                break
            }
            previous = next
        }
        guard let settled else {
            XCTFail(
                "\(name) never reached two identical frames in \(Self.settleAttempts) screenshots — "
                    + "capturing it would race whatever is still moving (#1212)",
                file: file, line: line
            )
            return
        }
        let attachment = XCTAttachment(screenshot: settled)
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
        // The import landing computes its geometry up front, but both hero and profile are
        // `Canvas` drawings — wait for them rather than assume the title implies them.
        waitFor(app, "trackPreview.grid", "the imported route's hero did not draw")
        waitFor(app, "detail.elevationProfile", "the imported route's elevation profile did not draw")
        capture(app, name: "route-imported")

        app.buttons["detail.upload"].tap()
        let begin = app.buttons["upload.begin"]
        XCTAssertTrue(begin.waitForExistence(timeout: 5), "the upload confirmation did not appear")
        begin.tap()

        XCTAssertTrue(
            app.staticTexts["On the device"].waitForExistence(timeout: 20),
            "the route upload did not finish"
        )
        // The done state swaps the whole sheet body; its button is the last thing laid out.
        waitFor(app, "upload.done", timeout: 5, "the upload sheet's done state did not settle")
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

        // The synced line and the row it announces land in separate updates — the card is what the
        // screenshot is *about*, so wait for it before capturing, not after.
        let card = app.buttons["main.card.ride-grimsel-pass"]
        XCTAssertTrue(card.waitForExistence(timeout: 5), "the downloaded ride was not listed")
        // This capture wants the top bar's post-sync check, which the shipped app retires after two
        // seconds. `-OBCHoldSyncConfirmation` parks it, so assert it's actually up rather than
        // hoping the run beat the clock (#1212).
        XCTAssertEqual(
            sync.label, "Synced",
            "the post-sync confirmation should still be held for the capture"
        )
        capture(app, name: "rides-synced")

        card.tap()
        XCTAssertTrue(
            app.descendants(matching: .any)["detail.screen"].firstMatch.waitForExistence(timeout: 5),
            "the downloaded ride detail did not open"
        )
        XCTAssertTrue(app.staticTexts["4.9 km"].waitForExistence(timeout: 5))
        waitFor(app, "trackPreview.grid", "the real Grimsel geometry should be visible in the ride hero")
        // The one genuinely async element on this screen, and the cause of #1212: a tracked ride's
        // profile samples come from `transport.rideDetail`, so the card (and everything the card
        // pushes down) appears a beat after the stats do.
        waitFor(app, "detail.elevationProfile", "the ride's elevation profile did not arrive")
        waitFor(app, "detail.services", "the connected-services block did not lay out")
        capture(app, name: "ride-detail")
    }
}
