import Foundation
import Testing
import OBCDomain
import OBCFormats
@testable import OBCTransport

/// A ride leaves the app in two ways: as a planned route (`Ride.plannedRoute()`) and as a GPX
/// file. Both must keep the ridden line, so the distance another app or the device measures is
/// the ride's own.
@Suite struct RideToRouteTests {
    /// A winding 1 Hz climb with sensors and a pause, like a device tracklog.
    private static let ride: Ride = {
        let start = Date(timeIntervalSince1970: 1_790_000_000)
        let points = (0..<1_200).map { i in
            let t = Double(i)
            return RidePoint(
                timestamp: start.addingTimeInterval(t),
                coordinate: Coordinate(latitude: 46.60 + t * 0.00004, longitude: 8.50 + 0.002 * sin(t / 40)),
                elevationMeters: (1_400 + t * 0.4).rounded(),
                heartRate: 140, cadence: 85, power: 210,
                segmentStart: i == 600
            )
        }
        return Ride(
            summary: RideSummary(id: RideID("ride"), name: "Furka", date: start, distanceMeters: 0),
            points: points
        )
    }()

    private static func length(_ coordinates: [Coordinate]) -> Double {
        zip(coordinates, coordinates.dropFirst()).reduce(0) { $0 + $1.0.routeDistance(to: $1.1) }
    }

    @Test func plannedRouteKeepsLineAndElevationWithoutTimeOrSensors() {
        let route = Self.ride.plannedRoute()

        #expect(route.name == "Furka")
        #expect(route.points.map(\.coordinate) == Self.ride.points.map(\.coordinate))
        #expect(route.points.map(\.elevationMeters) == Self.ride.points.map(\.elevationMeters))
        #expect(route.waypoints.isEmpty)
        #expect(RouteStats.compute(from: route.points).elevationGainMeters > 400)
    }

    @Test func plannedRouteUploadsWithTheRideDistance() throws {
        let rideLength = Self.length(Self.ride.points.map(\.coordinate))
        let decoded = try RouteObjectCodec.decode(
            RouteObjectCodec.encode(Self.ride.plannedRoute(), name: "Furka")
        )

        #expect(decoded.name == "Furka")
        #expect(abs(Double(decoded.totalDistanceMeters) - rideLength) / rideLength < 0.01)
        #expect(decoded.totalAscentMeters > 400)
    }

    @Test func sharedGPXReadsBackWithTheRideDistance() throws {
        let rideLength = Self.length(Self.ride.points.map(\.coordinate))
        let gpx = try GPXRideEncoder().encode(Self.ride)
        let imported = try GPXRouteDecoder().decode(gpx)

        let importedLength = Self.length(imported.points.map(\.coordinate))
        #expect(abs(importedLength - rideLength) / rideLength < 0.01)
    }
}
