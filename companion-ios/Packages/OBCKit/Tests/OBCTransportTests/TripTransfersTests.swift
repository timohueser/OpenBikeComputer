import Testing
import Foundation
@testable import OBCDomain

/// The waypoints a join keeps, and the transfers between days.
struct TripTransfersTests {
    /// Planar metres east and north of a fixed origin at 46.5° N.
    private func coordinate(_ x: Double, _ y: Double = 0) -> Coordinate {
        Coordinate(latitude: 46.5 + y / 111_320, longitude: 8 + x / (111_320 * cos(46.5 * Double.pi / 180)))
    }

    /// A straight file east along y = 0, a point every 100 m.
    private func file(_ from: Double, _ to: Double) -> [RoutePoint] {
        stride(from: from, through: to, by: 100).map { RoutePoint(coordinate: coordinate($0)) }
    }

    /// Three days: 0–10 km, a train to 11 km, 11–20 km, 20–30 km.
    private var withTransfer: Trip {
        Trip.joining(
            [file(0, 10_000), file(11_000, 20_000), file(20_000, 30_000)],
            id: TripID("t"), name: "T", bikeType: .road, now: Date(timeIntervalSince1970: 0))
    }

    @Test
    func aJoinKeepsTheWaypointsOfEveryFile() {
        let spring = Waypoint(index: 0, name: "Spring", distanceAlongMeters: 500, coordinate: coordinate(500, 20))
        let hut = Waypoint(index: 0, name: "Hut", distanceAlongMeters: 300, coordinate: coordinate(20_300, 40))
        let hutAgain = Waypoint(index: 0, name: "Hut", distanceAlongMeters: 0, coordinate: coordinate(20_302, 41))
        let trip = Trip.joining(
            [file(0, 10_000), file(10_000, 20_000), file(20_000, 30_000)], waypoints: [[spring], [], [hut, hutAgain]],
            id: TripID("t"), name: "T", bikeType: .road, now: Date(timeIntervalSince1970: 0))
        #expect(trip.waypoints.map(\.name) == ["Spring", "Hut"], "a waypoint two files share is one stop")
        #expect(trip.waypoints.allSatisfy { $0.kind == .waypoint })
    }

    @Test
    func aTransferLabelStaysWithItsBoundary() {
        var trip = withTransfer
        #expect(trip.endsAtTransfer(0) && !trip.endsAtTransfer(1))
        trip.setTransfer(1, to: .bus)
        #expect(trip.dayEnds[1].transfer == nil, "no transfer to label")
        trip.setTransfer(0, to: .train)
        trip.append(file(30_000, 40_000))
        #expect(trip.dayEnds[0].transfer == .train, "a re-projection keeps it")
        trip.setTransfer(0, to: nil)
        #expect(trip.dayEnds[0].transfer == nil)
    }

    @Test
    func aDayAfterATransferStartsWhereItsPieceStarts() throws {
        var trip = withTransfer
        trip.startName = "Andermatt"
        trip.namePlace(0, to: "Göschenen")
        trip.namePlace(1, to: "Ulrichen")
        let starts = try (0..<3).map { try #require(trip.dayStart($0)) }
        #expect(starts.map(\.name) == ["Andermatt", nil, "Ulrichen"])
        #expect(starts[1].coordinate.distance(to: coordinate(11_000)) < 1)
        #expect(starts[2].coordinate.distance(to: coordinate(20_000)) < 1)
    }
}
