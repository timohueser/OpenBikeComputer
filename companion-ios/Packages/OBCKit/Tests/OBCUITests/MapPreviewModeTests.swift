import XCTest
import OBCTransport
@testable import OBCUI

/// The basemap-or-grid decision (#294), extracted from the view so it's testable
/// without a simulator (the issue's rule). The map only shows with both a network
/// path and real geometry; every other combination is the intended grid fallback.
final class MapPreviewModeTests: XCTestCase {
    func testMapOnlyWhenOnlineAndHasCoordinates() {
        XCTAssertEqual(MapPreviewMode.resolve(isOnline: true, hasCoordinates: true), .map)
        XCTAssertEqual(MapPreviewMode.resolve(isOnline: true, hasCoordinates: false), .grid)
        XCTAssertEqual(MapPreviewMode.resolve(isOnline: false, hasCoordinates: true), .grid)
        XCTAssertEqual(MapPreviewMode.resolve(isOnline: false, hasCoordinates: false), .grid)
    }

    @MainActor
    func testReachabilityStoreStartsOptimisticThenTracksTheSeam() async {
        let store = ReachabilityStore(ConstantReachability(false), initiallyOnline: true)
        XCTAssertTrue(store.isOnline, "optimistic until the first path update lands")

        store.start()
        // The store subscribes on a Task; give it a few hops to converge.
        for _ in 0..<50 where store.isOnline {
            try? await Task.sleep(nanoseconds: 2_000_000)
        }
        XCTAssertFalse(store.isOnline, "converges to the seam's value")
    }
}
