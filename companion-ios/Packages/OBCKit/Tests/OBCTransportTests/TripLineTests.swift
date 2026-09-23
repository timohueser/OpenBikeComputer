import Testing
import Foundation
@testable import OBCDomain

/// A trip is one line with day ends as places: join, the cut into days, re-projection after a
/// change, and reverse. Out-and-back lines and loops put the same place on the line twice, so
/// they pin that a day end stays on its own leg.
struct TripLineTests {
    /// Planar metres east and north of a fixed origin at 46.5° N.
    private func coordinate(_ x: Double, _ y: Double = 0) -> Coordinate {
        Coordinate(latitude: 46.5 + y / 111_320, longitude: 8 + x / (111_320 * cos(46.5 * Double.pi / 180)))
    }

    /// Points every 100 m along the given corners, with the elevation equal to x / 10.
    private func file(_ corners: [(Double, Double)]) -> [RoutePoint] {
        var points = [RoutePoint(coordinate: coordinate(corners[0].0, corners[0].1), elevationMeters: corners[0].0 / 10)]
        for (a, b) in zip(corners, corners.dropFirst()) {
            let steps = Int((hypot(b.0 - a.0, b.1 - a.1) / 100).rounded())
            for step in 1...steps {
                let t = Double(step) / Double(steps)
                let x = a.0 + (b.0 - a.0) * t, y = a.1 + (b.1 - a.1) * t
                points.append(RoutePoint(coordinate: coordinate(x, y), elevationMeters: x / 10))
            }
        }
        return points
    }

    private func join(_ files: [[RoutePoint]]) -> Trip {
        Trip.joining(files, id: TripID("t"), name: "T", bikeType: .road, now: Date(timeIntervalSince1970: 0))
    }

    private func distances(_ trip: Trip) -> [Double] { trip.dayEnds.map { ($0.distance / 10).rounded() * 10 } }

    // MARK: Join

    @Test
    func joiningMakesOneDayPerFileWithTheDayEndsOnTheBoundaries() {
        let files = [file([(0, 0), (1000, 0)]), file([(1000, 0), (3000, 0)]), file([(3000, 0), (3500, 0)])]
        let trip = join(files)

        #expect(trip.dayCount == 3)
        #expect(trip.pieceStarts.isEmpty, "files that meet continue one piece")
        #expect(trip.dayEnds.map(\.coordinate) == files.map { $0[$0.count - 1].coordinate })
        #expect(distances(trip) == [1000, 3000, 3500])
        #expect(trip.dayLines() == files, "the cut gives back each file")
    }

    @Test
    func aGapAtADayEndStartsTheNextDayWhereTheNextFileStarts() {
        let first = file([(0, 0), (1000, 0)])
        let second = file([(4400, 0), (6400, 0)])
        let trip = join([first, second])

        #expect(trip.pieceStarts == [first.count])
        #expect(distances(trip) == [1000, 3000], "the 3.4 km gap counts nothing")
        #expect(trip.dayLines() == [first, second], "no straight segment across the gap")
    }

    @Test
    func appendKeepsTheDaysBeforeIt() {
        var trip = join([file([(0, 0), (1000, 0)]), file([(1000, 0), (2000, 0)])])
        let before = trip.dayLines()
        trip.append(file([(2000, 0), (2500, 0)]))
        #expect(Array(trip.dayLines().prefix(2)) == before)
        #expect(trip.dayCount == 3)
    }

    @Test
    func theCutInterpolatesADayEndBetweenVertices() {
        let line = [
            RoutePoint(coordinate: coordinate(0), elevationMeters: 100),
            RoutePoint(coordinate: coordinate(1000), elevationMeters: 200),
        ]
        var trip = Trip(
            id: TripID("t"), name: "T", bikeType: .road, line: line,
            dayEnds: [DayEnd(coordinate: coordinate(250, 40), distance: 0), DayEnd(coordinate: coordinate(1000), distance: 0)],
            addedAt: Date())
        trip.reproject()

        let days = trip.dayLines()
        #expect(days.count == 2)
        #expect(days[0].count == 2 && days[1].count == 2)
        #expect(days[0][1] == days[1][0], "one day ends where the next starts")
        #expect(abs(days[0][1].elevationMeters! - 125) < 1)
        #expect(days[0][1].coordinate.routeDistance(to: coordinate(250)) < 1)
    }

    // MARK: Re-projection

    @Test
    func aDayEndFarFromTheLineIsDroppedAndANearOneIsKept() {
        let line = file([(0, 0), (3000, 0)])
        var trip = Trip(
            id: TripID("t"), name: "T", bikeType: .road, line: line,
            dayEnds: [
                DayEnd(coordinate: coordinate(1000, 300), name: "Near", distance: 1000),
                DayEnd(coordinate: coordinate(2000, 800), name: "Far", distance: 2000),
                DayEnd(coordinate: coordinate(3000), distance: 3000),
            ],
            addedAt: Date())

        let dropped = trip.reproject()

        #expect(dropped.map(\.name) == ["Far"])
        #expect(trip.dayEnds.map(\.name) == ["Near", nil])
        #expect(distances(trip) == [1000, 3000])
    }

    @Test
    func aGapInsideADayIsBridgedByAStraightSegment() {
        let first = file([(0, 0), (1000, 0)])
        let second = file([(1400, 0), (2400, 0)])
        var trip = join([first, second])
        // The day end at the gap goes; the gap is now inside the only day.
        trip = Trip(
            id: trip.id, name: trip.name, bikeType: trip.bikeType, line: trip.line,
            pieceStarts: trip.pieceStarts, dayEnds: [trip.dayEnds[1]], addedAt: trip.addedAt)
        #expect(trip.dayLines() == [first + second])
    }

    // MARK: Reverse

    /// Three days with named ends and a gap at the second day end.
    private var threeDays: Trip {
        var trip = join([
            file([(0, 0), (2000, 0)]),
            file([(2000, 0), (2000, 1500)]),
            file([(2500, 1500), (4500, 1500)]),
        ])
        trip.renameDay(0, to: "Andermatt")
        trip.renameDay(1, to: "Ulrichen")
        trip.renameDay(2, to: "Brig")
        return trip
    }

    @Test
    func reverseKeepsEveryDayEndAtItsPlace() {
        let trip = threeDays
        var reversed = trip
        reversed.reverse()

        #expect(reversed.dayCount == 3)
        #expect(Array(reversed.dayEnds.prefix(2).map(\.coordinate)) == [trip.dayEnds[1].coordinate, trip.dayEnds[0].coordinate])
        #expect(reversed.dayEnds.map(\.name) == ["Ulrichen", "Andermatt", nil], "the old start had no name")
        #expect(reversed.startName == "Brig")
        #expect(reversed.dayEnds[2].coordinate == trip.line[0].coordinate)
        #expect(reversed.key != trip.key)
        // Each reversed day is an old day ridden backwards, in reverse day order.
        let old = trip.dayLines().map { $0.map(\.coordinate) }
        let new = reversed.dayLines().map { $0.map(\.coordinate) }
        #expect(new == old.reversed().map { Array($0.reversed()) })
    }

    @Test
    func reversingTwiceGivesBackTheLineThePlacesAndTheNames() {
        var original = threeDays
        original.startName = "Realp"
        var trip = original
        trip.reverse()
        #expect(trip.dayEnds.last?.name == "Realp")
        trip.reverse()
        #expect(trip.line == original.line)
        #expect(trip.pieceStarts == original.pieceStarts)
        #expect(trip.dayEnds.map(\.name) == ["Andermatt", "Ulrichen", "Brig"])
        #expect(trip.startName == "Realp")
        #expect(trip.dayEnds.map(\.coordinate) == original.dayEnds.map(\.coordinate))
        #expect(distances(trip) == distances(original))
    }

    @Test
    func aFileShorterThanADayChangesNothing() {
        var trip = join([file([(0, 0), (1000, 0)]), file([(1000, 0), (2000, 0)])])
        let before = trip
        let tiny = [RoutePoint(coordinate: coordinate(2000)), RoutePoint(coordinate: coordinate(2000.6))]
        #expect(!Trip.isDay(tiny))
        #expect(trip.append(tiny).isEmpty)
        #expect(trip == before)
        #expect(join([file([(0, 0), (1000, 0)]), tiny]).dayCount == 1)
    }

    /// Out 2 km and back on the same road, with a day end at the same place on both legs.
    @Test
    func anOutAndBackKeepsEachDayEndOnItsOwnLeg() {
        var trip = join([
            file([(0, 0), (1000, 0)]),
            file([(1000, 0), (2000, 0), (1000, 0)]),
            file([(1000, 0), (0, 0)]),
        ])
        #expect(trip.dayEnds[0].coordinate == trip.dayEnds[1].coordinate)
        #expect(distances(trip) == [1000, 3000, 4000])

        trip.reproject()
        #expect(distances(trip) == [1000, 3000, 4000], "a re-projection keeps each end on its leg")

        trip.reverse()
        #expect(distances(trip) == [1000, 3000, 4000])
        #expect(trip.dayLines().map { $0.count } == [11, 21, 11])
    }

    @Test
    func aLoopKeepsItsLastDayEndAtTheLineEnd() {
        var trip = join([file([(0, 0), (1000, 0), (1000, 1000)]), file([(1000, 1000), (0, 1000), (0, 0)])])
        #expect(trip.line.first?.coordinate == trip.line.last?.coordinate)
        #expect(distances(trip) == [2000, 4000])

        trip.reverse()
        #expect(distances(trip) == [2000, 4000])
        #expect(trip.dayEnds[0].coordinate == coordinate(1000, 1000))
    }
}
