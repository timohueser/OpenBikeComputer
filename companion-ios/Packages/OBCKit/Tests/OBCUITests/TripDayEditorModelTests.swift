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
        let model = editor(trip([file(0, 10_000), file(10_000, 20_000, y: 500), file(20_000, 30_000, y: 500)]))
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
