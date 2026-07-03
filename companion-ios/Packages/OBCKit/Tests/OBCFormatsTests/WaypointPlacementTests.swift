import XCTest
import OBCDomain
@testable import OBCFormats

/// A non-finite route coordinate makes the cumulative `along` non-finite,
/// which would violate `sorted`'s strict-weak-ordering precondition and *trap
/// the process*. Import decoders reject such coordinates upstream, but `place`
/// must still not crash if any non-import caller hands it non-finite geometry.
final class WaypointPlacementTests: XCTestCase {
    func testNonFiniteGeometryDoesNotCrashTheSort() {
        let points = [
            RoutePoint(coordinate: Coordinate(latitude: 47.00, longitude: 11.0), elevationMeters: nil),
            RoutePoint(coordinate: Coordinate(latitude: .nan, longitude: 11.0), elevationMeters: nil),
            RoutePoint(coordinate: Coordinate(latitude: 47.02, longitude: 11.0), elevationMeters: nil),
        ]
        let raw = [
            RawWaypoint(name: "Tail", note: nil, coordinate: Coordinate(latitude: 47.02, longitude: 11.0)),
            RawWaypoint(name: "Head", note: nil, coordinate: Coordinate(latitude: 47.00, longitude: 11.0)),
        ]

        // Reaching the assertions at all is the pass — a precondition failure
        // would abort the test process, not throw.
        let placed = WaypointPlacement.place(raw, along: points)
        XCTAssertEqual(placed.count, 2)
        XCTAssertEqual(placed.map(\.index), [0, 1], "re-indexed in the NaN-safe sorted order")
        // The finite (head) placement sorts ahead of the non-finite (tail) one.
        XCTAssertEqual(placed.first?.name, "Head")
    }
}
