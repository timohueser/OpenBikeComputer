import XCTest
import OBCDomain
@testable import OBCFormats

/// The shared waypoint projection's NaN safety. A non-finite route coordinate makes the
/// cumulative `along` non-finite, which violates `sorted`'s strict-weak-ordering precondition and
/// traps the process. The import decoders reject such coordinates, but `place` must not crash.
final class WaypointPlacementTests: XCTestCase {
    func testNonFiniteGeometryDoesNotCrashTheSort() {
        // A NaN middle point poisons the cumulative distance, so the waypoint
        // nearest the tail projects to a NaN `along`.
        let points = [
            RoutePoint(coordinate: Coordinate(latitude: 47.00, longitude: 11.0), elevationMeters: nil),
            RoutePoint(coordinate: Coordinate(latitude: .nan, longitude: 11.0), elevationMeters: nil),
            RoutePoint(coordinate: Coordinate(latitude: 47.02, longitude: 11.0), elevationMeters: nil),
        ]
        let raw = [
            RawWaypoint(name: "Tail", note: nil, coordinate: Coordinate(latitude: 47.02, longitude: 11.0)),
            RawWaypoint(name: "Head", note: nil, coordinate: Coordinate(latitude: 47.00, longitude: 11.0)),
        ]

        // Reaching the assertions at all is the pass: a precondition failure aborts the process.
        let placed = WaypointPlacement.place(raw, along: points)
        XCTAssertEqual(placed.count, 2)
        XCTAssertEqual(placed.map(\.index), [0, 1], "re-indexed in the NaN-safe sorted order")
        // The finite (head) placement sorts ahead of the non-finite (tail) one.
        XCTAssertEqual(placed.first?.name, "Head")
    }
}
