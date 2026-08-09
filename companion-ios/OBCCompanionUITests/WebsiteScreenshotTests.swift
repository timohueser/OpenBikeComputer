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
///   2. `-OBCHoldConfirmations` parks the timed states two of these shots are *of* — the post-sync
///      check (2 s) and the upload sheet that closes itself (2.6 s) — instead of leaving the
///      capture a few seconds to hit them in. Both are asserted still-held **after** their capture;
///   3. every page waits for the elements it draws **asynchronously** — a ride's elevation profile
///      arrives with the `rideDetail` read, not with the summary, and the device name and battery
///      arrive after the connect chain, so waiting on the static copy alone could screenshot a
///      half-formed screen. This is the load-bearing mechanism;
///   4. `capture` then waits for two consecutive identical frames, because the render can trail the
///      accessibility tree by a beat on a slow CI runner. Note what this does *not* do: a screen
///      whose async work hasn't started yet is perfectly still, and settles instantly. Only (3)
///      knows which frame is the right one.
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
            "-OBCHoldConfirmations",
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

    /// The device identity every page wears: the top bar's name, the battery, and — on the tracked
    /// empty state — the "Rides you record on Trailhead…" line. None of it is there at launch.
    /// `MainScreenModel.deviceName` starts as "Your OBC" and is only assigned after a **two-op**
    /// chain off the connected edge (`stampDeviceClock()` then `deviceInfo()`, each a mock round
    /// trip), and the battery arrives on its own stream. Settling can't catch this: work that
    /// hasn't started yet holds perfectly still. So wait for the real values.
    @MainActor
    private func waitForDeviceIdentity(
        _ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line
    ) {
        XCTAssertTrue(
            app.staticTexts["Trailhead"].waitForExistence(timeout: 15),
            "the device name never replaced the \"Your OBC\" placeholder",
            file: file, line: line
        )
        // The battery cluster ignores its children, so the percent is only readable through the
        // element's own label ("Battery 82 percent" for the fixture device).
        let battery = app.descendants(matching: .any)["topbar.battery"].firstMatch
        XCTAssertTrue(battery.waitForExistence(timeout: 10), "the battery indicator is missing", file: file, line: line)
        let charged = expectation(
            for: NSPredicate(format: "label == %@", "Battery 82 percent"), evaluatedWith: battery
        )
        wait(for: [charged], timeout: 15)
    }

    /// Screenshot the app once it has stopped changing: keep capturing until two consecutive frames
    /// are pixel-identical, then attach that frame. This proves the frame stopped changing between
    /// two samples — the identifier waits above are what prove it's the *right* frame.
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
        let upload = app.buttons["detail.upload"]
        XCTAssertTrue(upload.waitForExistence(timeout: 5))
        // The CTA reads "Upload to <device>", off the same unwaited `deviceName` the top bar uses —
        // so this page has the identity race too, even though it never shows the top bar.
        let named = expectation(
            for: NSPredicate(format: "label == %@", "Upload to Trailhead"), evaluatedWith: upload
        )
        wait(for: [named], timeout: 15)
        waitFor(app, "trackPreview.grid", "the imported route's hero did not draw")
        // Belt: an imported route computes its profile synchronously in `RouteDetailModel.init`, so
        // this can't currently fail — it's here so that if the import path ever grows an async
        // stage, the capture waits for it instead of quietly photographing a shorter screen.
        waitFor(app, "detail.elevationProfile", "the imported route's elevation profile did not draw")
        capture(app, name: "route-imported")

        upload.tap()
        let begin = app.buttons["upload.begin"]
        XCTAssertTrue(begin.waitForExistence(timeout: 5), "the upload confirmation did not appear")
        begin.tap()

        XCTAssertTrue(
            app.staticTexts["On the device"].waitForExistence(timeout: 20),
            "the route upload did not finish"
        )
        // The done state swaps the whole sheet body; its button is the last thing laid out.
        waitFor(app, "upload.done", timeout: 5, "the upload sheet's done state did not settle")
        // Its body names the device too. The 20 s upload wait already implies the identity chain
        // landed, but say so rather than rely on it.
        let onDeviceLine = app.staticTexts
            .matching(NSPredicate(format: "label CONTAINS %@", "Trailhead")).firstMatch
        XCTAssertTrue(
            onDeviceLine.waitForExistence(timeout: 10),
            "the done sheet still names the placeholder device"
        )
        capture(app, name: "route-on-device")
        // This sheet closes *itself* 2.6 s after `.done` unless `-OBCHoldConfirmations` parks it —
        // without the hold the capture above is of the main screen behind it. Assert the sheet
        // survived rather than let a wrong page reach the drift check.
        XCTAssertTrue(
            app.buttons["upload.done"].exists,
            "the upload sheet dismissed itself during the capture — the confirmation hold is broken"
        )
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
        // This page is not the still life it looks like: the top bar's name and battery, and the
        // empty state's own "Rides you record on Trailhead…" line, all arrive after the connect.
        waitForDeviceIdentity(app)
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
        // seconds. `-OBCHoldConfirmations` parks it, so assert it's actually up rather than
        // hoping the run beat the clock (#1212).
        XCTAssertEqual(
            sync.label, "Synced",
            "the post-sync confirmation should still be held for the capture"
        )
        capture(app, name: "rides-synced")
        // And still up afterwards: the settle loop spends real time, so an expiry mid-capture would
        // otherwise photograph the idle arrow and read as drift.
        XCTAssertEqual(
            sync.label, "Synced",
            "the post-sync confirmation expired during the capture — the confirmation hold is broken"
        )

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
        // Belt, like the import landing's profile wait: the services block is static markup on the
        // tracked dressing, so it's already there — kept so a future async services state can't
        // slip under the capture.
        waitFor(app, "detail.services", "the connected-services block did not lay out")
        capture(app, name: "ride-detail")
    }
}
