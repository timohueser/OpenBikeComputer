import Testing
import Foundation
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// TR7 host-side flows: the create & file paths — multi-select grouping
/// (ordering = as listed, not selection order; ≤ 1-trip invariant via the
/// store), filing per a picker `TripSelection` (the import row + route menus),
/// and moving a route between trips. Driven through `MainScreenModel` against
/// the `trips` fixture, exactly as the composition root wires it.
@MainActor
struct TripFlowModelTests {
    private let tripID = TripID("driftless-weekender")
    private let stageA = RouteID("devils-lake-overnighter")   // filed
    private let stageB = RouteID("cross-plains-gravel")       // filed
    // Loose routes, in `addedAt` (fixture) order: newest → oldest.
    private let kettle = RouteID("kettle-moraine-loop")
    private let sugar = RouteID("sugar-river-trail")
    private let blueMounds = RouteID("blue-mounds-backroads")

    /// A started model over the TR6/TR7 trips fixture: one trip (2 stages) + 3
    /// loose routes, seeded into an in-memory library like the composition root.
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

    /// Group orders stages **as listed** (newest `addedAt` first), not by the
    /// order the routes were selected in.
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

        // Both routes left the top level; the trip card took their place.
        let ids = model.plannedItems.map(\.id)
        #expect(!ids.contains("route:kettle-moraine-loop"))
        #expect(!ids.contains("route:blue-mounds-backroads"))
        #expect(ids.contains("trip:\(newTrip!.rawValue)"))
    }

    /// Grouping a route that's already in another trip strips it from that trip
    /// (the ≤ 1-trip invariant, enforced by the store on save).
    @Test
    func groupEnforcesTheOneTripInvariantViaTheStore() {
        let (model, _) = makeModel()

        // devils-lake is a driftless stage; grouping it with a loose route moves it.
        let newTrip = model.groupIntoTrip([kettle, stageA], name: "Mixed")
        #expect(model.trip(newTrip!)?.stageIDs == [kettle, stageA])
        // The old trip lost that stage but keeps the other.
        #expect(model.trip(tripID)?.stageIDs == [stageB])
    }

    /// An empty selection creates nothing (no empty trips).
    @Test
    func groupWithNoResolvableRoutesCreatesNothing() {
        let (model, _) = makeModel()
        #expect(model.groupIntoTrip([], name: "Nope") == nil)
        #expect(model.groupIntoTrip([RouteID("ghost")], name: "Nope") == nil)
        #expect(model.trips.count == 1)  // only the fixture trip
    }

    /// A blank name falls back to "New trip" (the locked default).
    @Test
    func groupBlankNameFallsBackToNewTrip() {
        let (model, _) = makeModel()
        let id = model.groupIntoTrip([kettle, sugar], name: "   ")
        #expect(model.trip(id!)?.name == "New trip")
    }

    // MARK: Import filing — with / without a trip selection

    /// `.none` (the import row's default) files nothing: the route stays loose.
    @Test
    func fileRouteNoneLeavesTheRouteLoose() {
        let (model, _) = makeModel()
        model.fileRoute(kettle, into: .none)
        #expect(model.trips.count == 1)  // no new trip
        #expect(model.tripContaining(kettle) == nil)
        #expect(model.plannedItems.map(\.id).contains("route:kettle-moraine-loop"))
    }

    /// `.new` starts a trip with the route as its first stage.
    @Test
    func fileRouteNewStartsATripWithTheRoute() {
        let (model, library) = makeModel()
        model.fileRoute(kettle, into: .new("Overnighter"))

        let trip = model.trips.first { $0.name == "Overnighter" }
        #expect(trip?.stageIDs == [kettle])
        #expect(model.tripContaining(kettle) == trip?.id)
        #expect(library.trips().contains { $0.name == "Overnighter" })
    }

    /// `.existing` files the route as the trip's **last** stage.
    @Test
    func fileRouteExistingAppendsAsLastStage() {
        let (model, _) = makeModel()
        model.fileRoute(kettle, into: .existing(tripID))
        #expect(model.trip(tripID)?.stageIDs == [stageA, stageB, kettle])
    }

    /// Filing a route already in the target trip is a no-op (no duplicate stage).
    @Test
    func fileRouteExistingIsIdempotentForAMemberAlreadyThere() {
        let (model, _) = makeModel()
        model.fileRoute(stageA, into: .existing(tripID))
        #expect(model.trip(tripID)?.stageIDs == [stageA, stageB])
    }

    // MARK: Move between trips (implicit remove)

    /// Filing a filed route into a different trip moves it — the invariant makes
    /// the move an implicit remove from the old trip.
    @Test
    func moveBetweenTripsRemovesFromTheOldTrip() {
        let (model, _) = makeModel()
        // A second trip to move a stage into.
        let target = model.groupIntoTrip([sugar], name: "Target")!

        model.fileRoute(stageA, into: .existing(target))

        #expect(model.trip(target)?.stageIDs == [sugar, stageA])  // appended
        #expect(model.trip(tripID)?.stageIDs == [stageB])          // removed from old
        #expect(model.tripContaining(stageA) == target)
    }

    /// Moving the last stage out of a trip dissolves it (the old trip empties).
    @Test
    func moveDissolvesAnEmptiedSourceTrip() {
        let (model, _) = makeModel()
        let solo = model.groupIntoTrip([sugar], name: "Solo")!

        // Move sugar into driftless — solo is now empty and dissolves.
        model.fileRoute(sugar, into: .existing(tripID))

        #expect(model.trip(solo) == nil)
        #expect(model.trip(tripID)?.stageIDs == [stageA, stageB, sugar])
    }

    // MARK: Remove from trip

    /// Remove-from-trip returns a route to the top level; the record survives.
    @Test
    func removeFromTripReturnsRouteToTopLevel() {
        let (model, _) = makeModel()
        model.removeRouteFromTrip(stageA)

        #expect(model.trip(tripID)?.stageIDs == [stageB])
        #expect(model.tripContaining(stageA) == nil)
        #expect(model.routes.contains { $0.id == stageA })  // record untouched
        #expect(model.plannedItems.map(\.id).contains("route:devils-lake-overnighter"))
    }

    /// Remove-from-trip on the last stage dissolves the trip (keeps the route).
    @Test
    func removeFromTripDissolvesOnLastStage() {
        let (model, _) = makeModel()
        model.removeRouteFromTrip(stageA)
        model.removeRouteFromTrip(stageB)

        #expect(model.trip(tripID) == nil)
        #expect(model.trips.isEmpty)
        #expect(model.routes.count == 5)  // no route deleted
    }

    /// Remove-from-trip on a loose route is a harmless no-op.
    @Test
    func removeFromTripNoOpForLooseRoute() {
        let (model, _) = makeModel()
        model.removeRouteFromTrip(kettle)
        #expect(model.trips.count == 1)
        #expect(model.trip(tripID)?.stageIDs == [stageA, stageB])
    }

    // MARK: Picker projection

    /// `tripPickerItems` mirrors the trips with name + stage count.
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
