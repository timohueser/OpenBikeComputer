import XCTest

final class OnboardingFlowTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    @MainActor
    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = [
            "-OBCScenario", "onboarding", "-OBCNetwork", "offline",
            "-OBCHideMockHUD", "-OBCDisableAnimations", "-OBCHoldConfirmations",
            "-AppleLanguages", "(en)", "-AppleLocale", "en_US"
        ]
        app.launchEnvironment["TZ"] = "UTC"
        app.launch()
        return app
    }

    @MainActor
    private func capture(_ app: XCUIApplication, _ name: String) {
        var previous = app.screenshot()
        for _ in 0..<40 {
            let next = app.screenshot()
            if previous.pngRepresentation == next.pngRepresentation {
                let attachment = XCTAttachment(screenshot: next)
                attachment.name = "onboarding-\(name)"
                attachment.lifetime = .keepAlways
                add(attachment)
                return
            }
            previous = next
        }
        XCTFail("\(name) did not settle")
    }

    @MainActor
    private func tap(_ app: XCUIApplication, _ identifier: String) {
        let button = app.buttons[identifier]
        XCTAssertTrue(button.waitForExistence(timeout: 15), identifier)
        let enabled = expectation(for: NSPredicate(format: "enabled == true"), evaluatedWith: button)
        wait(for: [enabled], timeout: 15)
        button.tap()
    }

    @MainActor
    func testDemoRouteAndRideReachLibrary() {
        let app = launch()
        tap(app, "onboarding.getStarted")
        tap(app, "pair.start")
        tap(app, "onboarding.allowBluetooth")
        XCTAssertTrue(app.staticTexts["pair.pairedTitle"].waitForExistence(timeout: 15))
        XCTAssertEqual(app.textFields["pairing.name"].value as? String, "OBC-7A2F")
        capture(app, "P06-factory-name")
        tap(app, "pairing.keepName")
        tap(app, "onboarding.sensorsContinue")
        tap(app, "onboarding.demoRoute")
        XCTAssertTrue(app.buttons["upload.done"].waitForExistence(timeout: 30))
        XCTAssertTrue(app.staticTexts["Grimsel Pass is on OBC-7A2F"].exists)
        capture(app, "P10-route-sent")
        tap(app, "upload.done")
        XCTAssertTrue(app.staticTexts["onboarding.rideTitle"].waitForExistence(timeout: 10))
        tap(app, "onboarding.syncRide")
        let title = app.staticTexts["onboarding.rideTitle"]
        let synced = expectation(
            for: NSPredicate(format: "label == 'Your demo ride is here'"), evaluatedWith: title)
        wait(for: [synced], timeout: 30)
        capture(app, "P11-ride-synced")
        tap(app, "onboarding.finish")
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["Grimsel Pass"].firstMatch.waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts.containing(
            NSPredicate(format: "label CONTAINS %@", "Demo ride, excluded from totals")
        ).firstMatch.exists)
        capture(app, "P12-demo-library")
        app.buttons["Got it"].tap()
        XCTAssertFalse(app.descendants(matching: .any)["onboarding.readyNote"].firstMatch.exists)
    }

    @MainActor
    func testBrowseThenReplaySetupFromSettings() {
        let app = launch()
        tap(app, "onboarding.browse")
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        tap(app, "topbar.settings")
        let replay = app.buttons["settings.replaySetup"]
        if !replay.isHittable { app.swipeUp() }
        tap(app, "settings.replaySetup")
        XCTAssertTrue(app.staticTexts["onboarding.welcomeTitle"].waitForExistence(timeout: 10))
    }
}
