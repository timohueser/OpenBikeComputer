import Testing
import Foundation
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// Stops near a day end: the request budget of the finder's cache, the sheet's lists online and
/// offline, and a pick that ends the day at a stop.
@MainActor
struct TripStopsModelTests {
    /// Planar metres east and north of a fixed origin at 46.5° N.
    private func coordinate(_ x: Double, _ y: Double = 0) -> Coordinate {
        Coordinate(latitude: 46.5 + y / 111_320, longitude: 8 + x / (111_320 * cos(46.5 * Double.pi / 180)))
    }

    private func file(_ from: Double, _ to: Double) -> [RoutePoint] {
        stride(from: from, through: to, by: 100).map { RoutePoint(coordinate: coordinate($0)) }
    }

    private let campsite = Stop(name: "Camp Ulrichen", coordinate: Coordinate(latitude: 46.5, longitude: 8.1), kind: .campsite)

    /// Two days of 10 km, east along y = 0, with a spring 200 m off the line at km 9.
    private func trip() -> Trip {
        let spring = Waypoint(index: 0, name: "Spring", distanceAlongMeters: 9_000, coordinate: coordinate(9_000, 200))
        return Trip.joining(
            [file(0, 10_000), file(10_000, 20_000)], waypoints: [[spring]],
            id: TripID("t"), name: "T", bikeType: .road, now: Date(timeIntervalSince1970: 0))
    }

    @Test
    func tenMovesOverTheSameFiveKilometresMakeAtMostFiveRequests() async throws {
        let search = MockStopSearch(stops: [])
        let finder = StopFinder(search: search)
        let line = MeasuredLine(routePoints: file(0, 30_000))
        for distance in [10_000.0, 14_900, 11_200, 13_800, 10_500, 14_400, 12_100, 12_900, 11_700, 13_300] {
            _ = try await finder.stops(near: distance, on: line)
        }
        #expect(await search.requestCount == 5)
    }

    @Test
    func aFailedRequestIsAskedAgain() async {
        let search = MockStopSearch(isOffline: true)
        let finder = StopFinder(search: search)
        let line = MeasuredLine(routePoints: file(0, 30_000))
        for _ in 0..<2 { _ = try? await finder.stops(near: 5_000, on: line) }
        #expect(await search.requestCount == 2)
    }

    @Test
    func offlineTheSheetListsTheWaypointsAndAsksNothing() async {
        let search = MockStopSearch(stops: [campsite])
        let model = TripStopsModel(
            trip: trip(), day: 0, finder: StopFinder(search: search), isOnline: false, onPick: { _ in })
        await model.load()
        #expect(model.nearby == .offline)
        #expect(model.stops.map(\.stop.name) == ["Spring"])
        #expect(await search.requestCount == 0)
    }

    @Test
    func onlineTheSheetAddsAppleMapsStopsInLineOrder() async {
        let hotel = Stop(name: "Hotel Furka", coordinate: coordinate(10_500, 400), kind: .hotel, mapItemID: "I2")
        let far = Stop(name: "Far away", coordinate: coordinate(10_000, 9_000), kind: .hotel)
        let model = TripStopsModel(
            trip: trip(), day: 0, finder: StopFinder(search: MockStopSearch(stops: [hotel, far])), isOnline: true,
            onPick: { _ in })
        await model.load()
        #expect(model.nearby == .loaded)
        #expect(model.stops.map(\.stop.name) == ["Spring", "Hotel Furka"])
        #expect(abs(model.stops[1].offset - 400) < 1)
    }

    @Test
    func pickingAnOnLineStopMovesAndNamesTheDayEnd() async throws {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.loadFixtures("trips")
        let library = InMemoryLibraryStore()
        control.seedLibrary(into: library)
        // On the loop's last segment, 1.5 km before the day end.
        let camp = Stop(
            name: "Camp Baraboo", coordinate: Coordinate(latitude: 43.42995, longitude: -89.745), kind: .campsite)
        let model = MainScreenModel(
            transport: MockTransport(control: control), library: library, stopSearch: MockStopSearch(stops: [camp]))
        model.start()
        let id = TripID("driftless-weekender")
        let before = try #require(model.trip(id)).dayEnds[0]

        let sheet = try #require(model.tripStops(id, day: 0, isOnline: true))
        await sheet.load()
        let placed = try #require(sheet.stops.first)
        #expect(placed.stop == camp)
        #expect(placed.isOnLine)
        sheet.pick(placed)

        let end = try #require(model.trip(id)).dayEnds[0]
        #expect(end.name == "Camp Baraboo")
        #expect(abs(end.distance - placed.distance) < 1)
        #expect(end.distance < before.distance - 1_000)
        #expect(library.trips().first { $0.id == id }?.dayEnds[0] == end)
        #expect(model.tripStops(id, day: 1, isOnline: true) == nil, "the last day ends at the line end")
    }
}
