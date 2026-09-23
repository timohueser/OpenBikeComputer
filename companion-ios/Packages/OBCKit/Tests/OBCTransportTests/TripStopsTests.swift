import Testing
import Foundation
@testable import OBCDomain

/// Stops against the trip line: the waypoints a join keeps, a stop's place along the line and its
/// offset, and a day that ends at a stop.
struct TripStopsTests {
    /// Planar metres east and north of a fixed origin at 46.5° N.
    private func coordinate(_ x: Double, _ y: Double = 0) -> Coordinate {
        Coordinate(latitude: 46.5 + y / 111_320, longitude: 8 + x / (111_320 * cos(46.5 * Double.pi / 180)))
    }

    /// A straight file east along y = 0, a point every 100 m.
    private func file(_ from: Double, _ to: Double) -> [RoutePoint] {
        stride(from: from, through: to, by: 100).map { RoutePoint(coordinate: coordinate($0)) }
    }

    private func stop(_ name: String, _ x: Double, _ y: Double) -> Stop {
        Stop(name: name, coordinate: coordinate(x, y), kind: .campsite, mapItemID: "id-\(name)")
    }

    /// Three days: 0–10 km, 10–20 km, 20–30 km.
    private func trip(waypoints: [[Waypoint]] = []) -> Trip {
        Trip.joining(
            [file(0, 10_000), file(10_000, 20_000), file(20_000, 30_000)], names: ["Stage 1"], waypoints: waypoints,
            id: TripID("t"), name: "T", bikeType: .road, now: Date(timeIntervalSince1970: 0))
    }

    @Test
    func aJoinKeepsTheWaypointsOfEveryFile() {
        let spring = Waypoint(index: 0, name: "Spring", distanceAlongMeters: 500, coordinate: coordinate(500, 20))
        let hut = Waypoint(index: 0, name: "Hut", distanceAlongMeters: 300, coordinate: coordinate(20_300, 40))
        let hutAgain = Waypoint(index: 0, name: "Hut", distanceAlongMeters: 0, coordinate: coordinate(20_302, 41))
        let trip = trip(waypoints: [[spring], [], [hut, hutAgain]])
        #expect(trip.waypoints.map(\.name) == ["Spring", "Hut"], "a waypoint two files share is one stop")
        #expect(trip.waypoints.allSatisfy { $0.kind == .waypoint })
    }

    @Test
    func placeGivesTheDistanceAlongAndTheOffset() {
        let placed = trip().place([stop("Hotel", 9_600, 400), stop("Camp", 10_300, 50)], near: 10_000)
        #expect(placed.map(\.stop.name) == ["Hotel", "Camp"])
        #expect(abs(placed[0].distance - 9_600) < 1)
        #expect(abs(placed[0].offset - 400) < 1)
        #expect(!placed[0].isOnLine)
        #expect(placed[1].isOnLine)
    }

    @Test
    func endingADayAtAStopMovesAndNamesTheDayEnd() {
        var trip = trip()
        let camp = trip.place([stop("Camp Ulrichen", 11_200, 400)], near: 10_000)[0]

        let ended = trip.endDay(0, at: camp)
        #expect(ended)
        let end = trip.dayEnds[0]
        #expect(abs(end.distance - 11_200) < 1)
        #expect(end.coordinate.distance(to: coordinate(11_200)) < 1, "the day end sits on the line")
        #expect(end.name == "Camp Ulrichen")
        #expect(end.title == "Stage 1", "the day keeps its own name")
        #expect(end.stop == camp.stop)
        #expect(abs((end.stopOffset ?? 0) - 400) < 1)
        let dropped = trip.reproject()
        #expect(dropped.isEmpty, "a day end at a stop stays through a line change")
        #expect(trip.dayEnds[0].stop == camp.stop)
    }

    @Test
    func aDayWithoutItsOwnNameTakesTheStopsName() {
        var trip = trip()
        let hut = trip.place([stop("Hut", 21_000, 0)], near: 20_000)[0]
        trip.endDay(1, at: hut)
        #expect(trip.dayEnds[1].title == nil, "no name of its own, so the day reads \"to Hut\"")
        #expect(trip.dayEnds[1].name == "Hut")
    }

    @Test
    func aStopPastTheNextDayEndOrOnTheLastDayEndsNothing() {
        var trip = trip()
        let before = trip.dayEnds
        let pastNext = trip.endDay(0, at: trip.place([stop("Far", 25_000, 0)])[0])
        let lastDay = trip.endDay(2, at: trip.place([stop("End", 29_000, 0)])[0])
        #expect(!pastNext)
        #expect(!lastDay, "the last day ends at the line end")
        #expect(trip.dayEnds == before)
    }

    @Test
    func aDayEndAtATransferDoesNotMoveAndOneAtAJoinDoes() {
        func trip(gap: Double) -> Trip {
            Trip.joining(
                [file(0, 10_000), file(10_000 + gap, 20_000)],
                id: TripID("t"), name: "T", bikeType: .road, now: Date(timeIntervalSince1970: 0))
        }
        var transfer = trip(gap: 1_000)
        var join = trip(gap: 100)
        #expect(transfer.endsAtTransfer(0))
        #expect(!join.endsAtTransfer(0))
        let movedAtTransfer = transfer.endDay(0, at: transfer.place([stop("Camp", 9_000, 0)], near: 10_000)[0])
        let movedAtJoin = join.endDay(0, at: join.place([stop("Camp", 9_000, 0)], near: 10_000)[0])
        #expect(!movedAtTransfer)
        #expect(movedAtJoin)
    }

    @Test
    func aTransferLabelStaysWithItsBoundary() throws {
        // A train from 10 km to 11 km, after day 1.
        var trip = Trip.joining(
            [file(0, 10_000), file(11_000, 20_000), file(20_000, 30_000)],
            id: TripID("t"), name: "T", bikeType: .road, now: Date(timeIntervalSince1970: 0))
        #expect(abs(try #require(trip.transferMeters(after: 0)) - 1_000) < 2)
        #expect(trip.transferMeters(after: 1) == nil)
        trip.setTransfer(1, to: .bus)
        #expect(trip.dayEnds[1].transfer == nil, "no transfer to label")

        trip.setTransfer(0, to: .train)
        trip.append(file(30_000, 40_000))
        #expect(trip.dayEnds[0].transfer == .train, "a re-projection keeps it")
        trip.reverse()
        #expect(trip.dayEnds.map(\.transfer) == [nil, nil, .train, nil], "the boundary is now after day 3")
        #expect(trip.endsAtTransfer(2))
        trip.setTransfer(2, to: nil)
        #expect(trip.dayEnds[2].transfer == nil)
    }

    @Test
    func aReverseMovesATransferEndAcrossItsGap() throws {
        // A train from Göschenen (10 km) to Andermatt (11 km), after day 1.
        var trip = Trip.joining(
            [file(0, 10_000), file(11_000, 20_000), file(20_000, 30_000)],
            id: TripID("t"), name: "T", bikeType: .road, now: Date(timeIntervalSince1970: 0))
        trip.namePlace(0, to: "Göschenen")
        trip.setTransfer(0, to: .train)

        trip.reverse()
        #expect(trip.endsAtTransfer(1))
        let end = trip.dayEnds[1]
        #expect(end.transfer == .train)
        #expect(end.coordinate.distance(to: coordinate(11_000)) < 1, "the day now ends at Andermatt")
        #expect(end.name == nil)
        let start = try #require(trip.dayStart(2))
        #expect(start.coordinate.distance(to: coordinate(10_000)) < 1)
        #expect(start.name == "Göschenen", "the name stays with its place")

        trip.reverse()
        #expect(trip.dayEnds[0].name == "Göschenen")
        #expect(trip.dayEnds[0].coordinate.distance(to: coordinate(10_000)) < 1)
        #expect(trip.dayEnds[0].transfer == .train)
    }

    @Test
    func aDayAfterATransferStartsWhereItsPieceStarts() throws {
        var trip = Trip.joining(
            [file(0, 10_000), file(11_000, 20_000), file(20_000, 30_000)],
            id: TripID("t"), name: "T", bikeType: .road, now: Date(timeIntervalSince1970: 0))
        trip.startName = "Andermatt"
        trip.namePlace(0, to: "Göschenen")
        trip.namePlace(1, to: "Ulrichen")
        let starts = try (0..<3).map { try #require(trip.dayStart($0)) }
        #expect(starts.map(\.name) == ["Andermatt", nil, "Ulrichen"])
        #expect(starts[1].coordinate.distance(to: coordinate(11_000)) < 1)
        #expect(starts[2].coordinate.distance(to: coordinate(20_000)) < 1)
    }

    @Test
    func reverseKeepsTheStopWithItsPlace() {
        var trip = trip()
        let camp = trip.place([stop("Camp", 12_000, 100)], near: 10_000)[0]
        trip.endDay(0, at: camp)
        trip.reverse()
        #expect(trip.dayEnds[1].stop == camp.stop)
        #expect(abs(trip.dayEnds[1].distance - 18_000) < 1)
    }
}
