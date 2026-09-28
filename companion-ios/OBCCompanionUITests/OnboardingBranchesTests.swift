import XCTest

final class OnboardingBranchesTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    @MainActor
    private func launch(_ scenario: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = [
            "-OBCScenario", scenario, "-OBCNetwork", "offline",
            "-OBCHideMockHUD", "-OBCDisableAnimations", "-OBCHoldConfirmations",
            "-AppleLanguages", "(en)", "-AppleLocale", "en_US"
        ]
        app.launchEnvironment["TZ"] = "UTC"
        app.launch()
        tap(app, "onboarding.getStarted")
        tap(app, "pair.start")
        tap(app, "onboarding.allowBluetooth")
        return app
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
    private func reachUpdate(_ app: XCUIApplication) {
        XCTAssertTrue(app.staticTexts["pair.pairedTitle"].waitForExistence(timeout: 15))
        tap(app, "pairing.keepName")
        tap(app, "onboarding.sensorsContinue")
        XCTAssertTrue(app.staticTexts["onboarding.updateTitle"].waitForExistence(timeout: 15))
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
    func testSeveralOBCsRequireSelectingTheMatchingName() {
        let app = launch("onboardingNearby")
        let first = "pair.deviceRow.00000000-0000-0000-0000-000000000001"
        let second = "pair.deviceRow.00000000-0000-0000-0000-000000000002"
        XCTAssertTrue(app.buttons[first].waitForExistence(timeout: 15))
        XCTAssertTrue(app.buttons[second].exists)
        XCTAssertTrue(app.buttons[first].label.contains("OBC-7A2F"))
        XCTAssertTrue(app.buttons[second].label.contains("OBC-9C41"))
        XCTAssertFalse(app.staticTexts["pair.pairedTitle"].exists)
        capture(app, "P04-choose-your-OBC")
        tap(app, second)
        XCTAssertTrue(app.staticTexts["pair.pairedTitle"].waitForExistence(timeout: 15))
        XCTAssertEqual(app.textFields["pairing.name"].value as? String, "OBC-9C41")
        capture(app, "P06-selected-OBC")
        tap(app, "pairing.keepName")
        XCTAssertTrue(app.staticTexts["onboarding.sensorsTitle"].waitForExistence(timeout: 10))
    }

    @MainActor
    func testCompatibleUpdateCanWaitUntilLater() {
        let app = launch("onboardingUpdate")
        reachUpdate(app)
        XCTAssertTrue(app.staticTexts["v0.4.0 → v0.5.0"].waitForExistence(timeout: 15))
        XCTAssertEqual(app.staticTexts["onboarding.updateTitle"].label, "Update your OBC")
        XCTAssertTrue(app.buttons["onboarding.updateNow"].exists)
        XCTAssertEqual(app.buttons["onboarding.updateLater"].label, "Later")
        capture(app, "P09-update-or-later")
        tap(app, "onboarding.updateLater")
        XCTAssertTrue(app.staticTexts["onboarding.routeTitle"].waitForExistence(timeout: 10))
    }

    @MainActor
    func testIncompatibleFirmwareCanFinishSetupWithoutTransfers() {
        let app = launch("onboardingUpdateNeeded")
        reachUpdate(app)
        let title = app.staticTexts["onboarding.updateTitle"]
        let mismatch = expectation(
            for: NSPredicate(format: "label == 'An update is needed'"), evaluatedWith: title)
        wait(for: [mismatch], timeout: 15)
        XCTAssertTrue(app.staticTexts[
            "Routes and ride sync need newer OBC firmware. Riding still works, and you can finish setup now."
        ].exists)
        XCTAssertEqual(app.buttons["onboarding.updateLater"].label, "Finish setup")
        capture(app, "P13-update-needed")
        tap(app, "onboarding.updateLater")
        XCTAssertTrue(app.otherElements["main.screen"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.staticTexts["onboarding.routeTitle"].exists)
        XCTAssertFalse(app.staticTexts["onboarding.rideTitle"].exists)
    }
}
