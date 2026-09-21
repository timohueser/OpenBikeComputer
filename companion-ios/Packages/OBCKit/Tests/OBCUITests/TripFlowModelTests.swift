import Testing
import Foundation
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The trip create and file flows: multi-select grouping (stage order follows the list, not the
/// selection order; a route belongs to at most one trip), filing per a picker `TripSelection`,
/// and moving a route between trips. Driven through `MainScreenModel` on the `trips` fixture.
@MainActor
struct TripFlowModelTests {
    private let tripID = TripID("driftless-weekender")
    private let stageA = RouteID("devils-lake-overnighter")   // filed
    private let stageB = RouteID("cross-plains-gravel")       // filed
    // Loose routes in fixture `addedAt` order: newest first.
    private let kettle = RouteID("kettle-moraine-loop")
    private let sugar = RouteID("sugar-river-trail")
    private let blueMounds = RouteID("blue-mounds-backroads")

    /// A started model over the trips fixture: one trip with 2 stages and 3 loose routes, seeded
    /// into an in-memory library like the composition root.
    private func makeModel() -> (MainScreenModel, InMemoryLibraryStore) {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.loadFixtures("trips")
        let library = InMemoryLibraryStore()
        control.seedLibrary(into: library)
        let model = MainScreenModel(transport: MockTransport(control: control), library: library)
        model.start()
        return (model, library)
    }

    // MARK: Multi-select grouping (ordering + invariant)

    /// "As listed" means newest `addedAt` first.
    @Test
    func groupOrdersStagesAsListedNotBySelectionOrder() {
        let (model, library) = makeModel()

        // Select oldest-then-newest; the trip must still list newest first.
        let newTrip = model.groupIntoTrip([blueMounds, kettle], name: "Gravel Weekend")
        #expect(newTrip != nil)
        #expect(model.trip(newTrip!)?.stageIDs == [kettle, blueMounds])
        #expect(model.trip(newTrip!)?.name == "Gravel Weekend")
        // Persisted, not just the mirror.
        #expect(library.trips().first { $0.id == newTrip! }?.stageIDs == [kettle, blueMounds])

        let ids = model.plannedItems.map(\.id)
        #expect(!ids.contains("route:kettle-moraine-loop"))
        #expect(!ids.contains("route:blue-mounds-backroads"))
        #expect(ids.contains("trip:\(newTrip!.rawValue)"))
    }

    @Test
    func groupEnforcesTheOneTripInvariantViaTheStore() {
        let (model, _) = makeModel()

        // devils-lake is a driftless stage; grouping it with a loose route moves it.
        let newTrip = model.groupIntoTrip([kettle, stageA], name: "Mixed")
        #expect(model.trip(newTrip!)?.stageIDs == [kettle, stageA])
        #expect(model.trip(tripID)?.stageIDs == [stageB])
    }

    @Test
    func groupWithNoResolvableRoutesCreatesNothing() {
        let (model, _) = makeModel()
        #expect(model.groupIntoTrip([], name: "Nope") == nil)
        #expect(model.groupIntoTrip([RouteID("ghost")], name: "Nope") == nil)
        #expect(model.trips.count == 1)  // only the fixture trip
    }

    @Test
    func groupBlankNameFallsBackToNewTrip() {
        let (model, _) = makeModel()
        let id = model.groupIntoTrip([kettle, sugar], name: "   ")
        #expect(model.trip(id!)?.name == "New trip")
    }

    // MARK: Import filing — with / without a trip selection

    @Test
    func fileRouteNoneLeavesTheRouteLoose() {
        let (model, _) = makeModel()
        model.fileRoute(kettle, into: .none)
        #expect(model.trips.count == 1)  // no new trip
        #expect(model.tripContaining(kettle) == nil)
        #expect(model.plannedItems.map(\.id).contains("route:kettle-moraine-loop"))
    }

    @Test
    func fileRouteNewStartsATripWithTheRoute() {
        let (model, library) = makeModel()
        model.fileRoute(kettle, into: .new("Overnighter"))

        let trip = model.trips.first { $0.name == "Overnighter" }
        #expect(trip?.stageIDs == [kettle])
        #expect(model.tripContaining(kettle) == trip?.id)
        #expect(library.trips().contains { $0.name == "Overnighter" })
    }

    @Test
    func fileRouteExistingAppendsAsLastStage() {
        let (model, _) = makeModel()
        model.fileRoute(kettle, into: .existing(tripID))
        #expect(model.trip(tripID)?.stageIDs == [stageA, stageB, kettle])
    }

    @Test
    func fileRouteExistingIsIdempotentForAMemberAlreadyThere() {
        let (model, _) = makeModel()
        model.fileRoute(stageA, into: .existing(tripID))
        #expect(model.trip(tripID)?.stageIDs == [stageA, stageB])
    }

    // MARK: Move between trips (implicit remove)

    @Test
    func moveBetweenTripsRemovesFromTheOldTrip() {
        let (model, _) = makeModel()
        let target = model.groupIntoTrip([sugar], name: "Target")!

        model.fileRoute(stageA, into: .existing(target))

        #expect(model.trip(target)?.stageIDs == [sugar, stageA])  // appended
        #expect(model.trip(tripID)?.stageIDs == [stageB])          // removed from old
        #expect(model.tripContaining(stageA) == target)
    }

    @Test
    func moveDissolvesAnEmptiedSourceTrip() {
        let (model, _) = makeModel()
        let solo = model.groupIntoTrip([sugar], name: "Solo")!

        model.fileRoute(sugar, into: .existing(tripID))

        #expect(model.trip(solo) == nil)
        #expect(model.trip(tripID)?.stageIDs == [stageA, stageB, sugar])
    }

    // MARK: Remove from trip

    @Test
    func removeFromTripReturnsRouteToTopLevel() {
        let (model, _) = makeModel()
        model.removeRouteFromTrip(stageA)

        #expect(model.trip(tripID)?.stageIDs == [stageB])
        #expect(model.tripContaining(stageA) == nil)
        #expect(model.routes.contains { $0.id == stageA })  // record untouched
        #expect(model.plannedItems.map(\.id).contains("route:devils-lake-overnighter"))
    }

    @Test
    func removeFromTripDissolvesOnLastStage() {
        let (model, _) = makeModel()
        model.removeRouteFromTrip(stageA)
        model.removeRouteFromTrip(stageB)

        #expect(model.trip(tripID) == nil)
        #expect(model.trips.isEmpty)
        #expect(model.routes.count == 5)  // no route deleted
    }

    @Test
    func removeFromTripNoOpForLooseRoute() {
        let (model, _) = makeModel()
        model.removeRouteFromTrip(kettle)
        #expect(model.trips.count == 1)
        #expect(model.trip(tripID)?.stageIDs == [stageA, stageB])
    }

    // MARK: Picker projection

    @Test
    func tripPickerItemsProjectNameAndStageCount() {
        let (model, _) = makeModel()
        let items = model.tripPickerItems
        #expect(items.count == 1)
        #expect(items.first?.id == tripID)
        #expect(items.first?.name == "Driftless Weekender")
        #expect(items.first?.stageCount == 2)
    }
}
