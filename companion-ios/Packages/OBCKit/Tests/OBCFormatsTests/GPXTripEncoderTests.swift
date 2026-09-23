import Foundation
import Testing
import OBCDomain
@testable import OBCFormats

/// The trip to GPX encoder: the planned line as one track with one segment per day, which the
/// app's own GPX import reads back point for point.
struct GPXTripEncoderTests {
    /// Points every 100 m east of 46.5° N 8° E from `from` to `to` metres, with the elevation
    /// equal to x / 10.
    private func file(_ from: Double, _ to: Double) -> [RoutePoint] {
        stride(from: from, through: to, by: 100).map { x in
            RoutePoint(
                coordinate: Coordinate(latitude: 46.5, longitude: 8 + x / (111_320 * cos(46.5 * Double.pi / 180))),
                elevationMeters: x / 10)
        }
    }

    @Test func writesOneTrackWithOneSegmentPerDay() throws {
        // A gap between the second and the third file: the third day starts at its own piece.
        let trip = Trip.joining(
            [file(0, 1_000), file(1_000, 3_000), file(4_400, 5_000)],
            id: TripID("t"), name: "Alps & Jura", bikeType: .road, now: Date(timeIntervalSince1970: 0))

        let data = GPXTripEncoder.encode(trip)
        let xml = try #require(String(data: data, encoding: .utf8))

        #expect(xml.components(separatedBy: "<trk>").count == 2)
        #expect(xml.components(separatedBy: "<trkseg>").count == 4)
        #expect(!xml.contains("<time>"), "a planned line has no time")

        let route = try GPXRouteDecoder().decode(data)
        let days = trip.dayLines().flatMap { $0 }
        #expect(route.name == "Alps & Jura")
        #expect(route.points.count == days.count)
        for (read, planned) in zip(route.points, days) {
            #expect(read.coordinate.distance(to: planned.coordinate) < 0.2)
            #expect(read.elevationMeters == planned.elevationMeters.map { $0.rounded() })
        }
    }
}
