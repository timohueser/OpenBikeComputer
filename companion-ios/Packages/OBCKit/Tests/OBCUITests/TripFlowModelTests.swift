import Testing
import Foundation
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The trip create and edit flows through `MainScreenModel` on the `trips` fixture: grouping and
/// filing move routes into a trip's line, and the trip edits save and stamp the trip.
@MainActor
struct TripFlowModelTests {
    private let tripID = TripID("driftless-weekender")
    private let kettle = RouteID("kettle-moraine-loop")
    private let sugar = RouteID("sugar-river-trail")
    private let blueMounds = RouteID("blue-mounds-backroads")

    /// A started model over the trips fixture: one two-day trip and three routes, seeded into an
    /// in-memory library like the composition root.
    private func makeModel(
        now: @escaping () -> Date = Date.init,
        placeName: (@Sendable (Coordinate) async -> String?)? = nil
    ) -> (MainScreenModel, InMemoryLibraryStore) {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.loadFixtures("trips")
        let library = InMemoryLibraryStore()
        control.seedLibrary(into: library)
        let model = MainScreenModel(
            transport: MockTransport(control: control), library: library, placeName: placeName, now: now)
        model.start()
        return (model, library)
    }

    @Test
    func groupJoinsTheRoutesIntoOneTripAndTheyLeaveTheLibrary() {
        let (model, library) = makeModel()

        let id = model.groupIntoTrip([kettle, blueMounds], name: "Gravel Weekend")!
        let trip = model.trip(id)!
        #expect(Set(model.tripDays(id).map(\.name)) == ["Kettle Moraine Loop", "Blue Mounds Backroads"],
                "each day keeps its route's name")
        #expect(trip.name == "Gravel Weekend")
        #expect(trip.dayCount == 2)
        #expect(library.trips().contains { $0.id == id })
        #expect(!model.routes.contains { $0.id == kettle || $0.id == blueMounds })
        #expect(library.plannedRoutes().map(\.id) == [sugar])
        #expect(model.plannedItems.map(\.id).contains("trip:\(id.rawValue)"))
    }

    /// A trip made or grown from routes carries a plan: a drawn leg per day, and a transfer leg
    /// where a day starts away from where the last one ends.
    @Test
    func tripsFromRoutesCarryAPlanThatGrowsWithThem() throws {
        let (model, _) = makeModel()
        let days = model.tripDays(tripID).map(\.points)
        let id = try #require(model.createTrip(name: "Planned", files: days))
        let plan = try #require(model.trip(id)?.plan)
        #expect(plan.days == 2 && plan.mode == .trip && plan.routePoints.last?.leg == .drawn)
        #expect(model.appendToTrip(id, file: days[0]))
        let grown = try #require(model.trip(id)?.plan)
        #expect(grown.days == 3 && grown.routePoints.map(\.leg).suffix(2) == [.transfer, .drawn])
        #expect(model.tripPlan(for: id)?.name == "Planned")
    }

    @Test
    func aTripHasAtMostFourteenDays() throws {
        let (model, _) = makeModel()
        let day = try #require(model.tripDays(tripID).first?.points)
        #expect(model.createTrip(name: "Long", files: Array(repeating: day, count: 15)) == nil)
        #expect(model.tripNotice == "A trip has at most 14 days.")
        let id = try #require(model.createTrip(name: "Full", files: Array(repeating: day, count: 14)))
        model.tripNotice = nil
        #expect(!model.appendToTrip(id, file: day) && model.trip(id)?.dayCount == 14)
        #expect(model.tripNotice == "A trip has at most 14 days.")
    }

    /// A route whose plan gains a night becomes a trip of the same name, with the route's device
    /// copy as the copy of day 1.
    @Test
    func aRouteWithNightsBecomesATrip() throws {
        let (model, library) = makeModel()
        let record = try #require(library.plannedRoutes().first { $0.id == kettle })
        let line = record.route.points
        var plan = try #require(PlannerPlan.keptLine(line))
        plan = try #require(plan.appendingDay(line.reversed(), name: nil))
        let id = try #require(model.replaceRouteWithTrip(
            kettle, plan: plan, line: line + line.reversed(), pointIndices: [0, line.count - 1, 2 * line.count - 1], bikeType: .road))
        let trip = try #require(model.trip(id))
        #expect(trip.name == record.summary.name && trip.dayCount == 2 && trip.plan == plan)
        #expect(!library.plannedRoutes().contains { $0.id == kettle })
        #expect(trip.dayCopies.first??.link == record.deviceLink)
    }

    @Test
    func groupWithNoResolvableRoutesCreatesNothing() {
        let (model, _) = makeModel()
        #expect(model.groupIntoTrip([], name: "Nope") == nil)
        #expect(model.groupIntoTrip([RouteID("ghost")], name: "Nope") == nil)
        #expect(model.trips.count == 1)
    }

    @Test
    func groupBlankNameFallsBackToNewTrip() {
        let (model, _) = makeModel()
        let id = model.groupIntoTrip([kettle, sugar], name: "   ")
        #expect(model.trip(id!)?.name == "New trip")
    }

    @Test
    func fileRouteNoneLeavesTheRoute() {
        let (model, _) = makeModel()
        #expect(model.fileRoute(kettle, into: .none) == nil)
        #expect(model.trips.count == 1)
        #expect(model.routes.contains { $0.id == kettle })
    }

    @Test
    func fileRouteNewStartsAOneDayTrip() {
        let (model, _) = makeModel()
        let id = model.fileRoute(kettle, into: .new("Overnighter"))
        #expect(model.trip(id!)?.name == "Overnighter")
        #expect(model.trip(id!)?.dayCount == 1)
        #expect(!model.routes.contains { $0.id == kettle })
    }

    @Test
    func fileRouteExistingAppendsALastDay() {
        let (model, _) = makeModel()
        let before = model.trip(tripID)!
        #expect(model.fileRoute(sugar, into: .existing(tripID)) == tripID)
        let after = model.trip(tripID)!
        #expect(after.dayCount == 3)
        #expect(after.dayEnds.prefix(2).map(\.coordinate) == before.dayEnds.map(\.coordinate))
        #expect(!model.routes.contains { $0.id == sugar })
    }

    @Test
    func aRouteTooShortToBeADayStaysARouteAndTheRiderIsTold() {
        let (model, library) = makeModel()
        let tiny = [0.0, 0.00001].map { RoutePoint(coordinate: Coordinate(latitude: 43, longitude: -89 + $0)) }
        model.addImportedRoute(PlannedRouteRecord(
            summary: RouteSummary(id: RouteID("tiny"), name: "Tiny", distanceMeters: 1, elevationGainMeters: 0),
            route: ImportedRoute(points: tiny), sourceFileName: "tiny.gpx", sourceFileData: Data()))

        #expect(model.fileRoute(RouteID("tiny"), into: .existing(tripID)) == nil)
        #expect(model.trip(tripID)?.dayCount == 2)
        #expect(library.plannedRoutes().contains { $0.id == RouteID("tiny") })
        #expect(model.tripNotice == "\u{201C}Tiny\u{201D} is too short to be a day. It stays a route.")
    }

    @Test
    func editsStampTheTripAsLastEdited() {
        var clock = Date().addingTimeInterval(3_600)
        let (model, _) = makeModel(now: { clock })
        let other = model.groupIntoTrip([kettle], name: "Other")!
        #expect(model.tripPickerItems.first?.id == other)

        clock += 60
        model.setTripStartDay(tripID, to: CivilDay(daysSince1970: 20_725))
        #expect(model.tripPickerItems.first?.id == tripID)
        #expect(model.trip(tripID)?.startDay == CivilDay(daysSince1970: 20_725))
    }

    @Test
    func bikeTypeRidesIntoEveryDayRoute() {
        let (model, _) = makeModel()
        let road = model.tripDays(tripID).map(\.crc32)
        model.setTripBikeType(tripID, to: .touring)
        #expect(model.trip(tripID)?.bikeType == .touring)
        #expect(zip(road, model.tripDays(tripID).map(\.crc32)).allSatisfy { $0 != $1 })
    }

    @Test
    func renameDayNamesTheDayRoute() {
        let (model, _) = makeModel()
        model.renameTripDay(tripID, day: 0, to: "Baraboo")
        #expect(model.tripDays(tripID).first?.name == "Baraboo")
    }

    @Test
    func unnamedDayEndsTakeTheirPlaceName() async {
        let (model, _) = makeModel(placeName: { _ in "Mazomanie" })
        let points = model.tripDays(tripID).map(\.points)
        let id = model.createTrip(name: "Named", files: points, dayNames: ["Eagle", nil])!
        for _ in 0..<100 where model.trip(id)?.dayEnds.last?.name == nil { await Task.yield() }
        // Only the day without a name of its own is looked up.
        #expect(model.trip(id)?.dayEnds.map(\.name) == [nil, "Mazomanie"])
        #expect(model.tripDays(id).map(\.name) == ["Eagle", "Day 2 Mazomanie"])
    }

    @Test
    func tripPickerItemsProjectNameAndDayCount() {
        let (model, _) = makeModel()
        #expect(model.tripPickerItems == [TripPickerItem(id: tripID, name: "Driftless Weekender", dayCount: 2)])
    }
}
