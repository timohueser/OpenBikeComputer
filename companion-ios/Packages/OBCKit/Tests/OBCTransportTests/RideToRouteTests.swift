import Foundation
import Testing
import OBCDomain
import OBCFormats
@testable import OBCTransport

/// A ride leaves the app in two ways: as a planned route (`Ride.plannedRoute()`) and as a GPX
/// file. Both must keep the ridden line, so the distance another app or the device measures is
/// the ride's own.
@Suite struct RideToRouteTests {
    /// A winding 1 Hz climb with sensors and a pause at point 600, like a device tracklog.
    /// `jump` moves every point after the pause north, as a car transfer would.
    private static func ride(jumpDegrees jump: Double = 0) -> Ride {
        let start = Date(timeIntervalSince1970: 1_790_000_000)
        let points = (0..<1_200).map { i in
            let t = Double(i)
            return RidePoint(
                timestamp: start.addingTimeInterval(t),
                coordinate: Coordinate(
                    latitude: 46.60 + t * 0.00004 + (i >= 600 ? jump : 0),
                    longitude: 8.50 + 0.002 * sin(t / 40)
                ),
                elevationMeters: (1_400 + t * 0.4).rounded(),
                heartRate: 140, cadence: 85, power: 210,
                segmentStart: i == 600
            )
        }
        return Ride(
            summary: RideSummary(id: RideID("ride"), name: "Furka", date: start, distanceMeters: 0),
            points: points
        )
    }

    private static let ride = ride()

    /// The distance the device counts: every leg except the jump into a new segment.
    private static func ridden(_ ride: Ride) -> Double {
        zip(ride.points, ride.points.dropFirst()).reduce(0) {
            $1.1.segmentStart ? $0 : $0 + $1.0.coordinate.routeDistance(to: $1.1.coordinate)
        }
    }

    private static func length(_ coordinates: [Coordinate]) -> Double {
        zip(coordinates, coordinates.dropFirst()).reduce(0) { $0 + $1.0.routeDistance(to: $1.1) }
    }

    @Test func plannedRouteKeepsLineAndElevationWithoutTimeOrSensors() throws {
        let route = try #require(Self.ride.plannedRoute())

        #expect(route.name == "Furka")
        #expect(route.points.map(\.coordinate) == Self.ride.points.map(\.coordinate))
        #expect(route.points.map(\.elevationMeters) == Self.ride.points.map(\.elevationMeters))
        #expect(route.waypoints.isEmpty)
        #expect(RouteStats.compute(from: route.points).elevationGainMeters > 400)
    }

    @Test func plannedRouteUploadsWithTheRideDistance() throws {
        let rideLength = Self.ridden(Self.ride)
        let decoded = try RouteObjectCodec.decode(
            RouteObjectCodec.encode(try #require(Self.ride.plannedRoute()), name: "Furka")
        )

        #expect(decoded.name == "Furka")
        #expect(abs(Double(decoded.totalDistanceMeters) - rideLength) / rideLength < 0.01)
        #expect(decoded.totalAscentMeters > 400)
    }

    /// A 20 km jump at the pause is not ridden, and a route through it would count and navigate
    /// it, so the ride does not become a route.
    @Test func aRideWithALongGapIsNoRoute() {
        let gapped = Self.ride(jumpDegrees: 0.18)
        #expect(Self.length(gapped.points.map(\.coordinate)) - Self.ridden(gapped) > 19_000)
        #expect(gapped.plannedRoute() == nil)
    }

    /// A short gap, such as a GPS dropout at a tunnel, joins while it adds under 1 %.
    @Test func aRideWithAShortGapJoinsWithinOnePercent() throws {
        let gapped = Self.ride(jumpDegrees: 0.0003)
        let route = try #require(gapped.plannedRoute())
        let rideLength = Self.ridden(gapped)
        #expect(Self.length(route.points.map(\.coordinate)) - rideLength > 30)
        #expect(abs(Self.length(route.points.map(\.coordinate)) - rideLength) / rideLength < 0.01)
    }

    @Test func sharedGPXReadsBackWithTheRideDistance() throws {
        let rideLength = Self.ridden(Self.ride)
        let gpx = try GPXRideEncoder().encode(Self.ride)
        let imported = try GPXRouteDecoder().decode(gpx)

        let importedLength = Self.length(imported.points.map(\.coordinate))
        #expect(abs(importedLength - rideLength) / rideLength < 0.01)
    }
}
