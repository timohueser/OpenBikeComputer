import Testing
import Foundation
import OBCDomain
@testable import OBCUI

/// The state under the day editor: a drag is live in the stats and reaches the trip on release,
/// the stepper and Even out balance the days, split and join keep the handles aligned, undo
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
        _ trip: Trip, isSplitMode: Bool = false, stops: [Stop]? = nil,
        placeName: (@Sendable (Coordinate) async -> String?)? = nil,
        onSave: @escaping (Trip) -> Void = { _ in }
    ) -> TripDayEditorModel {
        TripDayEditorModel(
            trip: trip, isSplitMode: isSplitMode, finder: stops.map { StopFinder(search: FakeStopSearch(stops: $0)) },
            placeName: placeName, onSave: onSave)!
    }

    /// Three days: 0–10 km, 10–20 km, 20–30 km.
    private func threeDays() -> Trip {
        trip([file(0, 10_000), file(10_000, 20_000), file(20_000, 30_000)], names: ["A", "B", "C"])
    }

    /// Three days with a transfer at the end of Day 1: Day 2 starts 500 m north of where Day 1 ends.
    private func withTransfer() -> Trip {
        trip([file(0, 10_000), file(10_000, 20_000, y: 500), file(20_000, 30_000, y: 500)])
    }

    @Test
    func aDragIsLiveInTheStatsAndReachesTheTripOnRelease() {
        let model = editor(threeDays())
        #expect(model.stats.count == 3)
        #expect(!model.hasChanges)
        let first = model.handles.markers[0].id

        let window = model.handles.window
        model.handles.begin(first)
        #expect(model.selectedDay == 0, "a grab highlights the day")
        #expect(model.handles.window == window, "a grab moves no window")
        model.handles.move(first, to: 12_000)
        #expect(abs((model.live.days[0]?.distanceMeters ?? 0) - 12_000) < 1, "the live figures follow the finger")
        #expect(abs((model.live.days[1]?.distanceMeters ?? 0) - 8_000) < 1)
        #expect(model.live.days.keys.sorted() == [0, 1], "only the two days at the handle")
        #expect(abs(model.stats[0].distanceMeters - 10_000) < 1, "the committed figures wait for the release")
        #expect(abs(model.trip.dayEnds[0].distance - 10_000) < 1, "the trip waits for the release")
        #expect(!model.canUndo)

        model.handles.end()
        #expect(model.live.days.isEmpty)
        #expect(abs(model.stats[0].distanceMeters - 12_000) < 1)
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

    /// The stepper is split mode's alone: after that, the day count changes with Split and Join.
    @Test
    func theStepperWorksOnlyInSplitMode() {
        let model = editor(threeDays())
        model.setDayCount(5)
        #expect(model.trip.dayCount == 3)
        #expect(!model.canUndo)
    }

    /// Split cuts the day at the middle of its riding time, not of its distance: a day that
    /// climbs in its first half is cut before its middle kilometre.
    @Test
    func splitCutsADayAtTheMiddleOfItsRidingTime() {
        let flat = stride(from: 0.0, through: 10_000, by: 100).map { RoutePoint(coordinate: coordinate($0), elevationMeters: 0) }
        let climb = stride(from: 10_000.0, through: 20_000, by: 100).map {
            RoutePoint(coordinate: coordinate($0), elevationMeters: min($0 - 10_000, 5_000) * 0.1)
        }
        let model = editor(trip([flat, climb], names: ["A", "B"]))
        let ids = model.handles.markers.map(\.id)

        model.splitDay(1)
        #expect(model.trip.dayCount == 3)
        #expect(model.trip.dayEnds[1].distance > 10_000 && model.trip.dayEnds[1].distance < 15_000)
        #expect(abs(model.stats[1].duration - model.stats[2].duration) < 1, "two halves of equal riding time")
        #expect(model.trip.dayEnds.map(\.title) == ["A", nil, "B"], "the old day's name stays with its end")
        #expect(model.selectedDay == 1, "the new end's day is highlighted")
        #expect(model.handles.markers.count == 2)
        #expect(model.handles.markers[0].id == ids[0], "the old handle keeps its id")

        model.undo()
        #expect(model.trip.dayCount == 2, "a split is one undo step")
        #expect(!model.canSplit(-1) && !model.canSplit(5))
    }

    @Test
    func joinRemovesTheEndBetweenTwoDaysButNeverATransfer() {
        let model = editor(threeDays())
        let ids = model.handles.markers.map(\.id)
        model.joinDay(0)
        #expect(model.trip.dayCount == 2)
        #expect(abs(model.trip.dayEnds[0].distance - 20_000) < 1)
        #expect(model.handles.markers.map(\.id) == [ids[1]])

        let transfer = editor(withTransfer())
        transfer.joinDay(0)
        #expect(transfer.trip.dayCount == 3, "a day end at a transfer holds a gap")
        #expect(!transfer.canUndo)
    }

    @Test
    func aStopCalloutEndsTheNearestDayThatMay() {
        let model = editor(threeDays())
        let camp = PlacedStop(stop: Stop(name: "Camp", coordinate: coordinate(10_500, 50), kind: .campsite), distance: 10_500, offset: 50)
        #expect(model.handles.stopActionTitle(camp) == "End Day 1 here")
        model.handles.onStopAction(camp)
        #expect(model.trip.dayEnds[0].stop == camp.stop)
        #expect(abs(model.handles.markers[0].distance - 10_500) < 1)
        #expect(model.canUndo)

        // Day 1 ends at a transfer 500 m from the stop; the nearest end that may move is Day 2's.
        let transfer = editor(withTransfer())
        #expect(transfer.handles.stopActionTitle(camp) == "End Day 2 here")
        transfer.handles.onStopAction(camp)
        #expect(transfer.trip.dayEnds[0].stop == nil)
        #expect(transfer.trip.dayEnds[1].stop == camp.stop)
    }

    /// The snap pass lands after the stops arrive, even though the geocoder has written place
    /// names into the draft meanwhile.
    @Test
    func theSnapPassLandsBesideAPlaceName() async {
        let camp = Stop(name: "Camp", coordinate: coordinate(10_500, 50), kind: .campsite)
        let model = editor(trip([file(0, 30_000)]), isSplitMode: true, stops: [camp], placeName: { (_: Coordinate) async -> String? in "Somewhere" })
        model.setDayCount(3)
        #expect(model.trip.dayEnds[0].stop == nil, "the first pass knows no stops yet")
        await model.snapTask?.value
        #expect(model.trip.dayEnds[0].stop == camp)
        #expect(abs(model.trip.dayEnds[0].distance - 10_500) < 1)
        #expect(model.trip.dayEnds[0].name == "Camp")
        #expect(model.handles.markers[0].distance == model.trip.dayEnds[0].distance)
        #expect(model.handles.stops.map(\.stop) == [camp], "the map and the profile show the stop")
        model.undo()
        #expect(model.trip.dayCount == 1, "both passes are one undo step")
    }

    /// A snap pass that arrives while a finger holds a handle is dropped: the finger wins, and
    /// the drag goes on.
    @Test
    func theSnapPassNeverLandsOnAFinger() async {
        let camp = Stop(name: "Camp", coordinate: coordinate(10_500, 50), kind: .campsite)
        let model = editor(trip([file(0, 30_000)]), isSplitMode: true, stops: [camp])
        model.setDayCount(3)
        let first = model.handles.markers[0].id
        model.handles.begin(first)
        model.handles.move(first, to: 12_000)
        await model.snapTask?.value
        #expect(model.handles.activeID == first, "the drag goes on")
        #expect(model.trip.dayEnds[0].stop == nil, "the snap was dropped")
        model.handles.move(first, to: 13_000)
        model.handles.end()
        #expect(abs(model.trip.dayEnds[0].distance - 13_000) < 1)
        #expect(model.trip.dayEnds[0].stop == nil)
    }

    @Test
    func theStepperStopsWhereDaysWouldBeTooShort() {
        let model = editor(trip([file(0, 2_000)]), isSplitMode: true)
        let cap = Trip.maxSplitDays(forLength: model.trip.measuredLine.length)
        #expect(cap < Trip.maxSplitDays && cap > 1)
        model.setDayCount(30)
        #expect(model.trip.dayCount == cap)
        var trip = model.trip
        #expect(trip.reproject().isEmpty)
    }

    /// A drag frame reads figures and blockers that were worked out at the last change: 120
    /// frames on a nine-day line of 50,000 points cost no line walks.
    @Test
    func aDragFrameWalksNoLine() {
        let dense = stride(from: 0.0, through: 300_000, by: 6).map {
            RoutePoint(coordinate: coordinate($0), elevationMeters: $0 * 0.01)
        }
        let model = editor(trip([dense]), isSplitMode: true)
        model.setDayCount(9)
        let id = model.handles.markers[4].id
        let clock = ContinuousClock()
        let elapsed = clock.measure {
            model.handles.begin(id)
            for frame in 0..<120 {
                model.handles.move(id, to: 165_000 + Double(frame) * 20)
                for day in 0..<9 { _ = model.removeBlocker(day) }
                _ = model.stats
            }
            model.handles.end()
        }
        #expect(model.removeBlockers.count == 9)
        #expect(elapsed < .seconds(1), "120 frames took \(elapsed)")
    }

    @Test
    func aDayEndAtATransferIsFixed() {
        let model = editor(withTransfer())
        #expect(model.handles.markers[0].isFixed)
        #expect(!model.handles.markers[1].isFixed)
        #expect(!model.handles.begin(model.handles.markers[0].id))
        #expect(model.removeBlocker(0) == "This day ends at a transfer.")
    }

    @Test
    func doneHandsTheDraftBack() {
        var saved: Trip?
        let model = editor(threeDays(), onSave: { saved = $0 })
        model.renameDay(1, to: "Over the pass")
        #expect(saved == nil, "nothing is saved before Done")
        model.save()
        #expect(saved?.dayEnds[1].title == "Over the pass")
    }
}

/// Apple Maps, answering with the stops inside the asked radius.
private struct FakeStopSearch: StopSearch {
    let stops: [Stop]

    func stops(near center: Coordinate, radius: Double) async throws -> [Stop] {
        stops.filter { $0.coordinate.distance(to: center) <= radius }
    }

    func places(matching query: String, southWest: Coordinate, northEast: Coordinate) async throws -> [Stop] { [] }
}
