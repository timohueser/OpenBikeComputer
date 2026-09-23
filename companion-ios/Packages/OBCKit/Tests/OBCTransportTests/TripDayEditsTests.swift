import Testing
import Foundation
@testable import OBCDomain

/// The day editor's changes to a trip: split, even out, add, remove and move a day end, each
/// inside the trip's invariants.
struct TripDayEditsTests {
    /// Planar metres east and north of a fixed origin at 46.5° N.
    private func coordinate(_ x: Double, _ y: Double = 0) -> Coordinate {
        Coordinate(latitude: 46.5 + y / 111_320, longitude: 8 + x / (111_320 * cos(46.5 * Double.pi / 180)))
    }

    /// A straight file east along `y`, a point every 100 m.
    private func file(_ from: Double, _ to: Double, y: Double = 0) -> [RoutePoint] {
        stride(from: from, through: to, by: 100).map { RoutePoint(coordinate: coordinate($0, y)) }
    }

    private func trip(_ files: [[RoutePoint]], names: [String?] = []) -> Trip {
        Trip.joining(
            files, names: names, id: TripID("t"), name: "T", bikeType: .road,
            now: Date(timeIntervalSince1970: 0))
    }

    /// Three days: 0–10 km, 10–20 km, 20–30 km, each with its own name.
    private func threeDays() -> Trip {
        trip([file(0, 10_000), file(10_000, 20_000), file(20_000, 30_000)], names: ["A", "B", "C"])
    }

    /// Three days with a transfer at the end of Day 1: Day 2 starts 500 m north of where Day 1 ends.
    private func withTransfer() -> Trip {
        trip([file(0, 10_000), file(10_000, 20_000, y: 500), file(20_000, 30_000, y: 500)], names: ["A", "B", "C"])
    }

    private func camp(_ name: String, _ x: Double) -> PlacedStop {
        PlacedStop(stop: Stop(name: name, coordinate: coordinate(x, 50), kind: .campsite), distance: x, offset: 50)
    }

    @Test
    func splitMakesUnnamedDaysThatEndAtStops() {
        var trip = trip([file(0, 30_000)], names: ["Long file"])
        trip.namePlace(0, to: "Brig")
        let lineEnd = trip.dayEnds[0]
        trip.split(into: 3, candidates: [camp("Camp Ulrichen", 9_400)])
        #expect(trip.dayCount == 3)
        #expect(trip.dayEnds.map(\.title) == [nil, nil, nil], "the stepper's days have no name of their own")
        #expect(trip.dayEnds[0].name == "Camp Ulrichen")
        #expect(trip.dayEnds[0].stop?.name == "Camp Ulrichen")
        #expect(trip.dayEnds[0].coordinate.distance(to: coordinate(9_400)) < 1, "the day end sits on the line")
        #expect(abs(trip.dayEnds[1].distance - 20_000) < 1)
        #expect(trip.dayEnds[2].name == "Brig", "the line end keeps its place")
        #expect(trip.dayEnds[2].distance == lineEnd.distance)
        let dropped = trip.reproject()
        #expect(dropped.isEmpty)
    }

    @Test
    func evenOutKeepsTheCountTheNamesAndTheTransfers() {
        var trip = withTransfer()
        trip.moveDayEnd(1, to: 26_000)
        trip.evenOut(candidates: [])
        #expect(trip.dayCount == 3)
        #expect(trip.dayEnds.map(\.title) == ["A", "B", "C"])
        #expect(abs(trip.dayEnds[0].distance - 10_000) < 1, "the day end at the transfer stays")
        #expect(abs(trip.dayEnds[1].distance - 20_000) < 1, "the days after the transfer balance among themselves")
    }

    @Test
    func editsCarryTheTransferOfADayEnd() {
        var trip = withTransfer()
        trip.setTransfer(0, to: .train)
        trip.evenOut(candidates: [])
        #expect(trip.dayEnds[0].transfer == .train, "even out leaves the day end at the transfer as it is")
        trip.addDayEnd(at: 5_000)
        #expect(trip.dayEnds[1].transfer == .train, "the transfer moves with its day end")
        trip.moveDayEnd(2, to: 26_000)
        #expect(trip.dayEnds[1].transfer == .train)
        var stored = withTransfer()
        stored.replaceDayEnds(from: trip)
        #expect(stored.dayEnds.map(\.transfer) == [nil, .train, nil, nil], "Done carries it into the stored trip")
        var split = trip
        split.split(into: 2, candidates: [])
        #expect(split.dayEnds.map(\.transfer) == [nil, nil], "a split makes new days without transfers")
    }

    @Test
    func evenOutFromAPositionLeavesTheDaysBeforeIt() {
        var trip = threeDays()
        trip.moveDayEnd(0, to: 12_000)
        trip.evenOut(from: 12_000, candidates: [])
        #expect(trip.dayEnds[0].distance == 12_000)
        #expect(abs(trip.dayEnds[1].distance - 21_000) < 1, "the rest is split in two equal days")
    }

    @Test
    func addDayEndGoesWhereTheRiderPutIt() {
        var trip = threeDays()
        let day = trip.addDayEnd(at: 13_500)
        #expect(day == 1, "inside Day 2")
        #expect(trip.dayCount == 4)
        #expect(abs(trip.dayEnds[1].distance - 13_500) < 1)
        #expect(trip.dayEnds.map(\.title) == ["A", nil, "B", "C"], "the name stays with the day's end")
        #expect(trip.addDayEnd(at: 19_990) == 2, "held inside the day it cuts")
        #expect(abs(trip.dayEnds[2].distance - (20_000 - Trip.minimumDayMeters)) < 1)
        #expect(trip.addDayEnd(at: 40_000) == nil, "past the line end")
    }

    @Test
    func theDayThatCanEndAtAStopIsTheNearestOneThatMay() {
        let trip = withTransfer()
        #expect(trip.day(thatCanEndAt: camp("Camp", 19_500)) == 1)
        #expect(trip.day(thatCanEndAt: camp("Camp", 10_500)) == 1, "Day 1 ends at a transfer and cannot take it; Day 2 can")
        #expect(trip.day(thatCanEndAt: camp("Camp", 10_050)) == nil, "too close to the transfer for Day 2")
        #expect(trip.day(thatCanEndAt: camp("Camp", 25_000)) == 1)
        #expect(threeDays().day(thatCanEndAt: camp("Camp", 10_500)) == 0)
    }

    @Test
    func removeDayEndJoinsTheDayToTheNextOne() {
        var trip = threeDays()
        let removed = trip.removeDayEnd(1)
        #expect(removed)
        #expect(trip.dayEnds.map(\.title) == ["A", "C"])
        #expect(zip(trip.dayEnds.map(\.distance), [10_000, 30_000]).allSatisfy { abs($0 - $1) < 1 })
        #expect(trip.removeDayEndBlocker(1) == "The last day ends the trip.")
        let removedLast = trip.removeDayEnd(1)
        #expect(!removedLast)

        var transfer = withTransfer()
        #expect(transfer.removeDayEndBlocker(0) == "This day ends at a transfer.")
        let removedTransfer = transfer.removeDayEnd(0)
        #expect(!removedTransfer)
        #expect(transfer.dayCount == 3)
    }

    @Test
    func moveDayEndStaysInRangeAndLeavesItsStop() {
        var trip = threeDays()
        trip.endDay(0, at: camp("Camp", 9_400))
        let moved = trip.moveDayEnd(0, to: 25_000)
        #expect(moved)
        #expect(abs(trip.dayEnds[0].distance - (20_000 - Trip.minimumDayMeters)) < 1, "held before the next day end")
        #expect(trip.dayEnds[0].stop == nil)
        #expect(trip.dayEnds[0].name == nil)
        #expect(trip.dayEnds[0].title == "A")
        let movedLast = trip.moveDayEnd(2, to: 25_000)
        #expect(!movedLast, "the last day ends at the line end")

        var transfer = withTransfer()
        let movedTransfer = transfer.moveDayEnd(0, to: 12_000)
        #expect(!movedTransfer, "a day end at a transfer cannot move")
    }

    @Test
    func splitOfAShortFileSurvivesReprojection() {
        var trip = trip([file(0, 2_000)])
        let days = Trip.maxSplitDays(forLength: trip.measuredLine.length)
        trip.split(into: days, candidates: [])
        #expect(trip.dayCount == days)
        let dropped = trip.reproject()
        #expect(dropped.isEmpty)
        #expect(trip.dayCount == days)
    }

    @Test
    func replaceDayEndsTakesOnlyTheDayEnds() {
        var edited = threeDays()
        edited.name = "Edited copy"
        edited.moveDayEnd(0, to: 12_000)
        var current = threeDays()
        current.name = "Current"
        current.startDay = CivilDay(daysSince1970: 20_000)
        current.replaceDayEnds(from: edited)
        #expect(current.name == "Current")
        #expect(current.startDay == CivilDay(daysSince1970: 20_000))
        #expect(abs(current.dayEnds[0].distance - 12_000) < 1)
    }

    @Test
    func dayStatsCoverTheWholeLine() {
        let stats = threeDays().dayStats()
        #expect(stats.count == 3)
        #expect(abs(stats.reduce(0) { $0 + $1.distanceMeters } - 30_000) < 1)
        #expect(stats.allSatisfy { $0.duration > 0 })
    }
}
