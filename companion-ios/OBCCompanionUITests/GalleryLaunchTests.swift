import XCTest

/// B11 acceptance: `-OBCShowUIGallery` presents the component gallery so
/// screenshot review can drive it without navigating placeholder UI.
final class GalleryLaunchTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    func testGalleryOpensViaLaunchArgAndScrolls() {
        let app = XCUIApplication()
        app.launchArguments += ["-OBCShowUIGallery"]
        app.launch()

        // The gallery sheet is up and shows kit content (the sync-state rows of
        // the Device Top Bar section render several "Trailhead" labels).
        let gallery = app.otherElements["uiGallery"].firstMatch
        let deviceName = app.staticTexts["Trailhead"].firstMatch
        XCTAssertTrue(deviceName.waitForExistence(timeout: 10), "gallery did not present")
        XCTAssertTrue(gallery.exists || app.scrollViews.count > 0)

        // It scrolls end to end without crashing (all sections construct).
        for _ in 0..<6 { app.swipeUp(velocity: .fast) }
        XCTAssertEqual(app.state, .runningForeground)
    }
}
