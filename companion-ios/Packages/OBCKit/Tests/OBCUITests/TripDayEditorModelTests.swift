import Testing
import Foundation
import OBCDomain
@testable import OBCUI

/// The state under the day editor: a drag is live in the stats and reaches the trip on release,
/// the stepper and Even out balance the days, add and remove keep the handles aligned, undo
/// steps back one change, and Done hands the draft back.
@MainActor
struct TripDayEditorModelTests {
    /// Planar metres east and north of a fixed origin at 46.5° N.
    private func coordinate(_ x: Double, _ y: Double = 0) -> Coordinate {
        Coordinate(latitude: 46.5 + y / 111_320, longitude: 8 + x / (111_320 * cos(46.5 * Double.pi / 180)))
    }

    /// A straight file east along `y`, a point every 100 m, climbing 20 m per kilometre.
    private func file(_ from: Double, _ to: Double, y: Double = 0) -> [RoutePoint] {
        stride(from: from, through: to, by: 100).map { RoutePoint(coordinate: coordinate($0, y), elevationMeters: $0 * 0.02) }
    }

    private func trip(_ files: [[RoutePoint]], names: [String?] = []) -> Trip {
        Trip.joining(files, names: names, id: TripID("t"), name: "T", bikeType: .gravel, now: Date(timeIntervalSince1970: 0))
    }

    private func editor(
        _ trip: Trip, isSplitMode: Bool = false, onSave: @escaping (Trip) -> Void = { _ in }
    ) -> TripDayEditorModel {
        TripDayEditorModel(trip: trip, isSplitMode: isSplitMode, finder: nil, onSave: onSave)!
    }

    /// Three days: 0–10 km, 10–20 km, 20–30 km.
    private func threeDays() -> Trip {
        trip([file(0, 10_000), file(10_000, 20_000), file(20_000, 30_000)], names: ["A", "B", "C"])
    }

    @Test
    func aDragIsLiveInTheStatsAndReachesTheTripOnRelease() {
        let model = editor(threeDays())
        #expect(model.stats.count == 3)
        #expect(!model.hasChanges)
        let first = model.handles.markers[0].id

        model.handles.begin(first)
        model.handles.move(first, to: 12_000)
        #expect(abs(model.stats[0].distanceMeters - 12_000) < 1, "the day figures follow the finger")
        #expect(abs(model.stats[1].distanceMeters - 8_000) < 1)
        #expect(abs(model.trip.dayEnds[0].distance - 10_000) < 1, "the trip waits for the release")
        #expect(!model.canUndo)

        model.handles.end()
        #expect(abs(model.trip.dayEnds[0].distance - 12_000) < 1)
        #expect(model.trip.dayEnds[0].title == "A")
        #expect(model.hasChanges)
        #expect(model.canUndo)

        model.undo()
        #expect(abs(model.trip.dayEnds[0].distance - 10_000) < 1)
        #expect(model.handles.markers[0].distance == model.trip.dayEnds[0].distance, "the handle steps back with the trip")
        #expect(!model.hasChanges)
    }

    @Test
    func theStepperSplitsOneFileIntoBalancedDays() {
        let model = editor(trip([file(0, 30_000)], names: ["Long"]), isSplitMode: true)
        #expect(model.trip.dayCount == 1)
        #expect(model.handles.markers.isEmpty)
        #expect(!model.balancesByDistance)

        model.setDayCount(3)
        #expect(model.trip.dayCount == 3)
        #expect(model.handles.markers.count == 2)
        #expect(model.stats.count == 3)
        #expect(abs(model.trip.dayEnds[0].distance - 10_000) < 1)
        #expect(abs(model.trip.dayEnds[1].distance - 20_000) < 1)
        #expect(model.trip.dayEnds.allSatisfy { $0.title == nil })
        #expect(abs(model.averageDayDuration - model.stats[0].duration) < 5)

        model.setDayCount(0)
        #expect(model.trip.dayCount == 1, "held to at least one day")
        model.undo()
        #expect(model.trip.dayCount == 3, "one stepper tap is one undo step")
    }

    @Test
    func evenOutRebalancesAfterADrag() {
        let model = editor(threeDays())
        let first = model.handles.markers[0].id
        model.handles.begin(first)
        model.handles.move(first, to: 15_000)
        model.handles.end()

        model.evenOut()
        #expect(abs(model.trip.dayEnds[0].distance - 10_000) < 1)
        #expect(model.trip.dayEnds.map(\.title) == ["A", "B", "C"])
        model.undo()
        #expect(abs(model.trip.dayEnds[0].distance - 15_000) < 1)
    }

    @Test
    func addAndRemoveKeepTheHandlesAligned() {
        let model = editor(threeDays())
        let first = model.handles.markers[0].id
        model.handles.begin(first)
        model.handles.move(first, to: 7_000)
        model.handles.end()
        let ids = model.handles.markers.map(\.id)

        #expect(model.addDayEnd() == 1, "Day 2 (13 km) is the longest")
        #expect(model.trip.dayCount == 4)
        #expect(model.handles.markers.count == 3)
        #expect([model.handles.markers[0].id, model.handles.markers[2].id] == ids, "the old handles keep their ids")

        #expect(model.removeBlocker(3) != nil, "the last day ends the trip")
        model.removeDayEnd(1)
        #expect(model.trip.dayCount == 3)
        #expect(model.handles.markers.map(\.id) == ids)
        #expect(zip(model.handles.markers.map(\.distance), [7_000, 20_000]).allSatisfy { abs($0 - $1) < 1 })
    }

    @Test
    func aDayEndAtATransferIsFixed() {
        let model = editor(trip([file(0, 10_000), file(10_000, 20_000, y: 500), file(20_000, 30_000, y: 500)]))
        #expect(model.handles.markers[0].isFixed)
        #expect(!model.handles.markers[1].isFixed)
        #expect(!model.handles.begin(model.handles.markers[0].id))
        #expect(model.removeBlocker(0) == "This day ends at a transfer.")
    }

    @Test
    func doneHandsTheDraftBack() {
        var saved: Trip?
        let model = editor(threeDays()) { saved = $0 }
        model.renameDay(1, to: "Over the pass")
        #expect(saved == nil, "nothing is saved before Done")
        model.save()
        #expect(saved?.dayEnds[1].title == "Over the pass")
    }
}
