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
    func tenMovesOverTheSameFiveKilometresAskFourTimes() async throws {
        let search = MockStopSearch(stops: [])
        let finder = StopFinder(search: search)
        let line = MeasuredLine(routePoints: file(0, 30_000))
        for distance in [10_000.0, 14_900, 11_200, 13_800, 10_500, 14_400, 12_100, 12_900, 11_700, 13_300] {
            _ = try await finder.stops(near: distance, on: line)
        }
        #expect(await search.requestCount == 4)
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

    /// Apple Maps answers recorded at a real day end near Haslach im Kinzigtal. The town lies
    /// 3.4 km away; some answers lie beyond the radius.
    @Test
    func theSheetKeepsRecordedAppleMapsStopsWithinTheRadius() async throws {
        let url = try #require(Bundle.module.url(forResource: "haslach-stops", withExtension: "json", subdirectory: "Fixtures"))
        let recorded = try JSONDecoder().decode([RecordedStop].self, from: Data(contentsOf: url)).map(\.stop)
        // Two days along a meridian, the first ending at the recorded day end.
        let end = Coordinate(latitude: 48.2919, longitude: 8.126544)
        let day = { (from: Double) in
            stride(from: from, through: from + 11_000, by: 100).map {
                RoutePoint(coordinate: Coordinate(latitude: end.latitude - $0 / 111_320, longitude: end.longitude))
            }
        }
        let trip = Trip.joining(
            [day(-11_000), day(0)], id: TripID("t"), name: "T", bikeType: .gravel, now: Date(timeIntervalSince1970: 0))
        let model = TripStopsModel(
            trip: trip, day: 0, finder: StopFinder(search: MockStopSearch(stops: recorded)), isOnline: true,
            onPick: { _ in })
        await model.load()
        #expect(Set(model.stops.map(\.stop.name)) == [
            "Fuxxbau", "Landhaus Hechtsberg", "Wohnmobil Stellplatz", "Schlossberghof", "Ramsteinerhof",
            "Gasthaus Aiple", "Stadthotel Haslach", "Mosers Blume Relax & Genuss Hotel", "Haus Zum Hobel",
            "Ferienwohnung Hinterer Strickerhof", "Gasthof Blume", "Hohengasthaus Nillhofe",
            "Hausach Home Bahnhofsnähe", "Brucherhof", "Gasthaus Käppelehof",
        ], "Hotel-Restaurant Alte Bauernschanke lies 5.4 km away")
    }

    @Test
    func aSearchKeepsAppleMapsOrderAndMeasuresEachPlace() async {
        let far = Stop(name: "Camping Eggishorn", coordinate: coordinate(19_960, 600), kind: .campsite)
        let near = Stop(name: "Camping riverside", coordinate: coordinate(4_000, 500), kind: .campsite)
        let model = TripStopsModel(
            trip: trip(), day: 0, finder: StopFinder(search: MockStopSearch(stops: [far, near, campsite])),
            isOnline: true, onPick: { _ in })
        model.query = "camping "
        await model.search()
        #expect(model.results?.map(\.stop.name) == ["Camping Eggishorn", "Camping riverside"])
        #expect(model.results.map { $0.map { ($0.distance / 100).rounded() * 100 } } == [20_000, 4_000])
        #expect(model.results.map { $0.map(model.canPick) } == [false, true], "day 1 ends at least 100 m before day 2")
    }

    @Test
    func aSearchOnAnOutAndBackLandsOnTheDaysOwnLeg() async {
        // Out 10 km, back 10 km, out again: day 2 can end only on the way back.
        let trip = Trip.joining(
            [file(0, 10_000), file(0, 10_000).reversed(), file(-10_000, 0).reversed()],
            id: TripID("t"), name: "T", bikeType: .road, now: Date(timeIntervalSince1970: 0))
        let camp = Stop(name: "Camp", coordinate: coordinate(2_000, 30), kind: .campsite)
        let model = TripStopsModel(
            trip: trip, day: 1, finder: StopFinder(search: MockStopSearch(stops: [camp])), isOnline: true,
            onPick: { _ in })
        model.query = "Camp"
        await model.search()
        let placed = model.results?.first
        #expect(placed.map { ($0.distance / 10).rounded() * 10 } == 18_000)
        #expect(placed.map(model.canPick) == true)
    }

    /// A started model over the `trips` fixture, with Apple Maps answering `stops`.
    private func makeModel(stops: [Stop]) -> (MainScreenModel, InMemoryLibraryStore) {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.loadFixtures("trips")
        let library = InMemoryLibraryStore()
        control.seedLibrary(into: library)
        let model = MainScreenModel(
            transport: MockTransport(control: control), library: library, stopSearch: MockStopSearch(stops: stops))
        model.start()
        return (model, library)
    }

    @Test
    func pickingAnOnLineStopMovesAndNamesTheDayEnd() async throws {
        let camp = Stop(name: "Camp Baraboo", coordinate: coordinate(9_000, 50), kind: .campsite)
        let (model, library) = makeModel(stops: [camp])
        let id = try #require(model.createTrip(
            name: "T", files: [file(0, 10_000), file(10_000, 20_000)], dayNames: ["Stage 1"]))

        let sheet = try #require(model.tripStops(id, day: 0, isOnline: true))
        await sheet.load()
        let placed = try #require(sheet.stops.first)
        #expect(placed.stop == camp)
        #expect(placed.isOnLine)
        sheet.pick(placed)

        let end = try #require(model.trip(id)).dayEnds[0]
        #expect(end.name == "Camp Baraboo")
        #expect(end.title == "Stage 1", "the day keeps its file's own name")
        #expect(abs(end.distance - 9_000) < 1)
        #expect(library.trips().first { $0.id == id }?.dayEnds[0] == end)
        #expect(model.tripStops(id, day: 1, isOnline: true) == nil, "the last day ends at the line end")
    }

    @Test
    func aDayEndAtATransferListsStopsButDoesNotMove() async throws {
        // On the loop's last segment, 1.5 km before the day end; Day 2 starts 35 km away.
        let camp = Stop(
            name: "Camp Baraboo", coordinate: Coordinate(latitude: 43.42995, longitude: -89.745), kind: .campsite)
        let (model, _) = makeModel(stops: [camp])
        let id = TripID("driftless-weekender")
        let before = try #require(model.trip(id)).dayEnds

        let sheet = try #require(model.tripStops(id, day: 0, isOnline: true))
        await sheet.load()
        #expect(sheet.endsAtTransfer)
        let placed = try #require(sheet.stops.first)
        #expect(!sheet.canPick(placed))
        sheet.pick(placed)
        #expect(model.trip(id)?.dayEnds == before)
    }
}

/// One Apple Maps answer as the fixture stores it.
private struct RecordedStop: Decodable {
    var name: String
    var latitude: Double
    var longitude: Double
    var kind: String
    var mapItemID: String

    var stop: Stop {
        Stop(
            name: name, coordinate: Coordinate(latitude: latitude, longitude: longitude),
            kind: Stop.Kind(rawValue: kind) ?? .place, mapItemID: mapItemID)
    }
}
