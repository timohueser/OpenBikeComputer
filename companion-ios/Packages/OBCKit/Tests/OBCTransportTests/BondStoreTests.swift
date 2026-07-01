import XCTest
@testable import OBCTransport

final class BondStoreTests: XCTestCase {
    private var suiteName: String!
    private var defaults: UserDefaults!

    override func setUp() {
        super.setUp()
        suiteName = "obc-bond-tests-\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suiteName)
    }

    override func tearDown() {
        defaults.removePersistentDomain(forName: suiteName)
        super.tearDown()
    }

    func testRoundTrip() {
        let store = UserDefaultsBondStore(defaults: defaults)
        XCTAssertNil(store.load(), "fresh install: never bonded")

        store.save(BondRecord(deviceName: "Trailhead"))
        XCTAssertEqual(store.load(), BondRecord(deviceName: "Trailhead"))

        // Rename (H3) refreshes the record in place.
        store.save(BondRecord(deviceName: "Ridgeline"))
        XCTAssertEqual(store.load(), BondRecord(deviceName: "Ridgeline"))

        store.clear()
        XCTAssertNil(store.load(), "forget device (H2) empties the record")
    }
}
