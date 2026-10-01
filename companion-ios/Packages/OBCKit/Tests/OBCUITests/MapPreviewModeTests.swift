import XCTest
import Testing
import OBCTransport
@testable import OBCUI

/// The basemap-or-grid decision, extracted from the view so it is testable without a simulator.
/// The map shows only with both a network path and real geometry; every other combination is the
/// intended grid fallback.
final class MapPreviewModeTests: XCTestCase {
    func testMapOnlyWhenOnlineAndHasCoordinates() {
        XCTAssertEqual(MapPreviewMode.resolve(isOnline: true, hasCoordinates: true), .map)
        XCTAssertEqual(MapPreviewMode.resolve(isOnline: true, hasCoordinates: false), .grid)
        XCTAssertEqual(MapPreviewMode.resolve(isOnline: false, hasCoordinates: true), .grid)
        XCTAssertEqual(MapPreviewMode.resolve(isOnline: false, hasCoordinates: false), .grid)
    }

    @MainActor
    func testReachabilityStoreStartsOptimisticThenTracksTheSeam() async throws {
        let store = ReachabilityStore(ConstantReachability(false), initiallyOnline: true)
        XCTAssertTrue(store.isOnline, "optimistic until the first path update lands")

        store.start()
        try await waitFor("reachability update") { !store.isOnline }
        XCTAssertFalse(store.isOnline, "converges to the seam's value")
    }
}

@Suite("Native map availability")
struct NativeMapAvailabilityTests {
    @Test func failuresAndOfflineStatePersistAcrossCoverageChanges() {
        for outside in [false, true] {
            #expect(NativeMapLoadState.failed.message(outsideRegion: outside) == "Map unavailable. Tap to try again.")
            #expect(NativeMapLoadState.offline.message(outsideRegion: outside) == "Map needs an internet connection.")
        }
        #expect(NativeMapLoadState.ready.message(outsideRegion: false) == nil)
        #expect(NativeMapLoadState.ready.message(outsideRegion: true) == "Outside the available map region.")
    }
}
