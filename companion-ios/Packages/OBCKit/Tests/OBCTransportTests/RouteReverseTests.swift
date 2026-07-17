import Foundation
import Testing
import OBCDomain
@testable import OBCTransport

/// Reverse an imported route (#503) — the pure `ImportedRoute.reversed()`
/// transform and its end-to-end result through the real OBCR encoder. The
/// correctness details the issue calls out are pinned here: point order flips,
/// distance is unchanged, ascent and descent swap, the camera start moves to the
/// old end, and each waypoint keeps its coordinate while its `Distance Along`
/// becomes `total − along`, re-sorted ascending and re-indexed (spec §4).
@Suite struct RouteReverseTests {
    /// A monotonic climb north — every step clears the 2 m hysteresis, so the
    /// whole rise is confirmed ascent one way and the whole drop is descent the
    /// other: a clean swap to assert against.
    private static let climbingPoints = [
        RoutePoint(coordinate: Coordinate(latitude: 48.00, longitude: 8.0), elevationMeters: 100),
        RoutePoint(coordinate: Coordinate(latitude: 48.01, longitude: 8.0), elevationMeters: 150),
        RoutePoint(coordinate: Coordinate(latitude: 48.02, longitude: 8.0), elevationMeters: 220),
        RoutePoint(coordinate: Coordinate(latitude: 48.03, longitude: 8.0), elevationMeters: 300),
    ]

    private static func length(of points: [RoutePoint]) -> Double {
        guard points.count > 1 else { return 0 }
        return (1..<points.count).reduce(0.0) { sum, i in
            sum + points[i - 1].coordinate.distance(to: points[i].coordinate)
        }
    }

    // MARK: Geometry

    @Test func reversesPointOrder() {
        let route = ImportedRoute(name: "Climb", points: Self.climbingPoints)
        let reversed = route.reversed()
        #expect(reversed.points.map(\.coordinate) == Self.climbingPoints.reversed().map(\.coordinate))
        #expect(reversed.points.first?.coordinate == Self.climbingPoints.last?.coordinate)
        #expect(reversed.points.last?.coordinate == Self.climbingPoints.first?.coordinate)
        // Elevation rides with each point, not resampled.
        #expect(reversed.points.map(\.elevationMeters) == [300, 220, 150, 100])
    }

    @Test func carriesNameAndCreatorThrough() {
        let route = ImportedRoute(name: "Climb", creator: "Komoot", points: Self.climbingPoints)
        let reversed = route.reversed()
        #expect(reversed.name == "Climb")
        #expect(reversed.creator == "Komoot")
    }

    // MARK: Waypoints (spec §4)

    @Test func flipsWaypointDistanceAndReindexes() {
        let total = Self.length(of: Self.climbingPoints)
        let route = ImportedRoute(
            name: "Climb",
            points: Self.climbingPoints,
            waypoints: [
                Waypoint(index: 0, name: "Trailhead", distanceAlongMeters: 0,
                         coordinate: Coordinate(latitude: 48.00, longitude: 8.0)),
                Waypoint(index: 1, name: "Water", note: "spring",
                         distanceAlongMeters: total * 0.25,
                         coordinate: Coordinate(latitude: 48.008, longitude: 8.0)),
                Waypoint(index: 2, name: "Summit", distanceAlongMeters: total,
                         coordinate: Coordinate(latitude: 48.03, longitude: 8.0)),
            ]
        )
        let reversed = route.reversed()

        // Re-sorted into the new ride order: Summit (was at the end) now leads.
        #expect(reversed.waypoints.map(\.name) == ["Summit", "Water", "Trailhead"])
        // Re-indexed 0..<n along the new order.
        #expect(reversed.waypoints.map(\.index) == [0, 1, 2])
        // total − along for each; the endpoints trade places exactly.
        #expect(reversed.waypoints[0].distanceAlongMeters == 0)               // Summit: total − total
        #expect(abs(reversed.waypoints[1].distanceAlongMeters - total * 0.75) < 1e-6)
        #expect(abs(reversed.waypoints[2].distanceAlongMeters - total) < 1e-9) // Trailhead: total − 0
        // Coordinates and notes are untouched by the flip.
        #expect(reversed.waypoints[0].coordinate == Coordinate(latitude: 48.03, longitude: 8.0))
        #expect(reversed.waypoints[1].note == "spring")
    }

    @Test func waypointClampsPastLengthToZero() {
        let total = Self.length(of: Self.climbingPoints)
        // A waypoint projected a hair past the measured end (rounding) must not
        // flip to a negative distance.
        let route = ImportedRoute(
            name: "Climb", points: Self.climbingPoints,
            waypoints: [Waypoint(index: 0, name: "End", distanceAlongMeters: total + 5,
                                 coordinate: Coordinate(latitude: 48.03, longitude: 8.0))]
        )
        #expect(route.reversed().waypoints[0].distanceAlongMeters == 0)
    }

    @Test func noWaypointsReversesGeometryAlone() {
        let route = ImportedRoute(name: "Climb", points: Self.climbingPoints)
        let reversed = route.reversed()
        #expect(reversed.waypoints.isEmpty)
        #expect(reversed.points.count == Self.climbingPoints.count)
    }

    // MARK: Degenerate geometry

    @Test func emptyAndSinglePointDoNotCrash() {
        #expect(ImportedRoute(name: "Empty", points: []).reversed().points.isEmpty)
        let one = [RoutePoint(coordinate: Coordinate(latitude: 48, longitude: 8), elevationMeters: nil)]
        let reversed = ImportedRoute(name: "One", points: one).reversed()
        #expect(reversed.points.count == 1)
    }

    @Test func missingElevationReversesCleanly() {
        let points = [
            RoutePoint(coordinate: Coordinate(latitude: 48.00, longitude: 8.0), elevationMeters: nil),
            RoutePoint(coordinate: Coordinate(latitude: 48.01, longitude: 8.0), elevationMeters: nil),
            RoutePoint(coordinate: Coordinate(latitude: 48.02, longitude: 8.0), elevationMeters: nil),
        ]
        let reversed = ImportedRoute(name: "Flat", points: points).reversed()
        #expect(reversed.points.map(\.elevationMeters) == [nil, nil, nil])
        // Encodes without an elevation-driven trap; ascent/descent both zero.
        let decoded = try? RouteObjectCodec.decode(RouteObjectCodec.encode(reversed, name: "Flat"))
        #expect(decoded?.totalAscentMeters == 0)
        #expect(decoded?.totalDescentMeters == 0)
    }

    // MARK: End-to-end through the OBCR encoder (spec §1/§3/§4)

    @Test func encodedReverseSwapsAscentDescentAndStart() throws {
        let waypoints = [
            Waypoint(index: 0, name: "Trailhead", distanceAlongMeters: 0,
                     coordinate: Coordinate(latitude: 48.00, longitude: 8.0)),
            Waypoint(index: 1, name: "Summit",
                     distanceAlongMeters: Self.length(of: Self.climbingPoints),
                     coordinate: Coordinate(latitude: 48.03, longitude: 8.0)),
        ]
        let forward = ImportedRoute(name: "Climb", points: Self.climbingPoints, waypoints: waypoints)
        let reversed = forward.reversed()

        let f = try RouteObjectCodec.decode(RouteObjectCodec.encode(forward, name: "Climb"))
        let r = try RouteObjectCodec.decode(RouteObjectCodec.encode(reversed, name: "Climb"))

        // Distance is unchanged; ascent and descent trade places.
        #expect(r.totalDistanceMeters == f.totalDistanceMeters)
        #expect(f.totalAscentMeters > 0)
        #expect(r.totalAscentMeters == f.totalDescentMeters)
        #expect(r.totalDescentMeters == f.totalAscentMeters)

        // Camera start moves to the old end.
        #expect(abs(r.start.latitude - 48.03) < 1e-4)
        #expect(abs(f.start.latitude - 48.00) < 1e-4)

        // Waypoints ride back in reversed order with flipped distances; Summit,
        // now first, sits at the start.
        #expect(r.waypoints.map(\.name) == ["Summit", "Trailhead"])
        #expect(r.waypoints[0].distanceAlongMeters == 0)
        #expect(r.waypoints[1].distanceAlongMeters == Double(f.totalDistanceMeters))
    }

    // MARK: Name disambiguation

    @Test func reversedNameAppendsSuffix() {
        #expect(RouteReversal.reversedName("Kettle Loop") == "Kettle Loop (reversed)")
    }

    @Test func reversedNameFallsBackForBlank() {
        #expect(RouteReversal.reversedName("   ") == "Route (reversed)")
    }
}
