import XCTest

final class RidePhotoTests: XCTestCase {
    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    @MainActor
    func testPhotosRemainAvailableAfterDismissalAndAdding() {
        let app = XCUIApplication()
        app.launchArguments += [
            "-OBCScenario", "syncUpToDate", "-OBCFixtures", "effort",
            "-OBCPhotoAccess", "full", "-OBCNetwork", "offline",
            "-OBCHideMockHUD", "-OBCDisableAnimations",
        ]
        app.launch()
        XCTAssertTrue(app.buttons["Rides"].waitForExistence(timeout: 10))
        app.buttons["Rides"].tap()
        let ride = app.buttons["main.card.ride-gletsch-descent"]
        XCTAssertTrue(ride.waitForExistence(timeout: 10))
        ride.tap()

        let offer = app.descendants(matching: .any)["photos.offer"].firstMatch
        XCTAssertTrue(offer.waitForExistence(timeout: 10))
        reveal(offer.buttons["quietRow.dismiss"], in: app)
        offer.buttons["quietRow.dismiss"].tap()

        let addMore = app.buttons["photos.addMore"]
        XCTAssertTrue(addMore.waitForExistence(timeout: 5))
        addMore.tap()
        let firstPhoto = app.buttons["photos.pick.mock-photo-1"]
        XCTAssertTrue(firstPhoto.waitForExistence(timeout: 5))
        firstPhoto.tap()
        app.buttons["photos.add"].tap()

        XCTAssertTrue(addMore.waitForExistence(timeout: 5))
        reveal(addMore, in: app)
        addMore.tap()
        XCTAssertTrue(firstPhoto.waitForExistence(timeout: 5))
        XCTAssertEqual(app.buttons["photos.add"].label, "Add 1")
        app.buttons["photos.add"].tap()
        XCTAssertTrue(addMore.waitForExistence(timeout: 5))
    }

    @MainActor
    private func reveal(_ element: XCUIElement, in app: XCUIApplication) {
        for _ in 0..<6 {
            if element.isHittable { return }
            app.swipeUp()
        }
        XCTAssertTrue(element.isHittable)
    }
}
