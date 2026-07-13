import Testing
import Foundation
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// TR6 host-side model behavior: the Planned list's filed/loose partition, trip
/// stats, dissolve-on-last-stage-removal, and the two delete-dialog branches
/// (Ungroup vs Delete trip & routes). Pure `PlannedItem.partition` is tested
/// directly; the edits run through `MainScreenModel` against the `trips` fixture.
@MainActor
struct TripListModelTests {
    private let tripID = TripID("driftless-weekender")
    private let stageA = RouteID("devils-lake-overnighter")
    private let stageB = RouteID("cross-plains-gravel")

    /// A started model over the TR6 trips fixture: one trip (2 stages) + 3 loose
    /// routes, seeded into an in-memory library exactly as the composition root does.
    private func makeModel() -> (MainScreenModel, MockControl) {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.loadFixtures("trips")
        let library = InMemoryLibraryStore()
        control.seedLibrary(into: library)
        let model = MainScreenModel(transport: MockTransport(control: control), library: library)
        model.start()  // library-first content (trips + items) is set synchronously
        return (model, control)
    }

    // MARK: Partition (pure)

    @Test
    func partitionHidesFiledRoutesAndInterleavesByAddedAt() {
        let base = Date()
        func record(_ id: String, addedAt: Date) -> PlannedRouteRecord {
            PlannedRouteRecord(
                summary: RouteSummary(id: RouteID(id), name: id, distanceMeters: 1_000, elevationGainMeters: 100),
                route: ImportedRoute(points: []),
                sourceFileName: "\(id).gpx", sourceFileData: Data(), addedAt: addedAt)
        }
        let records = [
            record("loose-new", addedAt: base),                          // newest
            record("filed-1", addedAt: base.addingTimeInterval(-10)),
            record("filed-2", addedAt: base.addingTimeInterval(-20)),
            record("loose-old", addedAt: base.addingTimeInterval(-30)),  // oldest
        ]
        let trip = TripRecord(
            id: TripID("t"), name: "Trip", stageIDs: [RouteID("filed-1"), RouteID("filed-2")],
            addedAt: base.addingTimeInterval(-15))

        let items = PlannedItem.partition(records: records, trips: [trip])

        // Filed routes are not loose rows; the trip stands in for them.
        #expect(items.map(\.id) == ["route:loose-new", "trip:t", "route:loose-old"])
    }

    // MARK: List model — filed/loose from the fixture

    @Test
    func fixtureTripFilesItsStagesAndLeavesLooseRoutes() {
        let (model, _) = makeModel()

        #expect(model.trips.count == 1)
        #expect(model.trip(tripID)?.stageIDs == [stageA, stageB])

        // All five planned routes still resolve (a stage must open its detail).
        #expect(model.routes.count == 5)

        // The top-level list: the trip card + the 3 routes NOT in it.
        let ids = model.plannedItems.map(\.id)
        #expect(ids.contains("trip:driftless-weekender"))
        #expect(!ids.contains("route:devils-lake-overnighter"))  // filed
        #expect(!ids.contains("route:cross-plains-gravel"))      // filed
        #expect(ids.contains("route:kettle-moraine-loop"))       // loose
        #expect(ids.filter { $0.hasPrefix("route:") }.count == 3)
    }

    // MARK: Trip stats

    @Test
    func tripStatsSumTheMemberRoutes() {
        let (model, control) = makeModel()
        let members = control.fixtures.routes.filter { $0.summary.id == stageA || $0.summary.id == stageB }
        let expectedDistance = members.reduce(0) { $0 + $1.summary.distanceMeters }
        let expectedClimb = members.reduce(0) { $0 + $1.summary.elevationGainMeters }

        let stats = model.tripStats(tripID)
        #expect(stats.stageCount == 2)
        #expect(stats.distanceMeters == expectedDistance)
        #expect(stats.elevationGainMeters == expectedClimb)
    }

    // MARK: Dissolve on last remove

    @Test
    func removingTheLastStageDissolvesTheTripAndKeepsTheRoutes() {
        let (model, _) = makeModel()

        // First removal keeps the trip (still one stage) and returns the route loose.
        let dissolvedFirst = model.removeStage(stageA, from: tripID)
        #expect(dissolvedFirst == false)
        #expect(model.trip(tripID)?.stageIDs == [stageB])
        #expect(model.plannedItems.map(\.id).contains("route:devils-lake-overnighter"))

        // Second removal empties the trip → it dissolves; the route stays loose.
        let dissolvedSecond = model.removeStage(stageB, from: tripID)
        #expect(dissolvedSecond == true)
        #expect(model.trip(tripID) == nil)
        #expect(model.trips.isEmpty)
        // Both former stages are now top-level, and no route was deleted.
        #expect(model.routes.count == 5)
        let ids = model.plannedItems.map(\.id)
        #expect(ids.contains("route:devils-lake-overnighter"))
        #expect(ids.contains("route:cross-plains-gravel"))
    }

    // MARK: Reorder stages

    @Test
    func reorderPersistsTheNewRideOrder() {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.loadFixtures("trips")
        let library = InMemoryLibraryStore()
        control.seedLibrary(into: library)
        let model = MainScreenModel(transport: MockTransport(control: control), library: library)
        model.start()

        // Move stage B (index 1) before stage A — SwiftUI onMove semantics.
        model.reorderTripStages(tripID, from: IndexSet(integer: 1), to: 0)

        // The model's live order…
        #expect(model.trip(tripID)?.stageIDs == [stageB, stageA])
        #expect(model.tripStages(tripID).map(\.id) == [stageB, stageA])
        // …and the store's persisted order (a fresh read, not the mirror).
        #expect(library.trips().first { $0.id == tripID }?.stageIDs == [stageB, stageA])
    }

    // MARK: Delete-dialog composition

    @Test
    func ungroupDropsTheTripButKeepsEveryRoute() {
        let (model, _) = makeModel()

        model.ungroupTrip(tripID)

        #expect(model.trip(tripID) == nil)
        #expect(model.routes.count == 5)  // routes untouched
        let ids = model.plannedItems.map(\.id)
        #expect(ids.contains("route:devils-lake-overnighter"))
        #expect(ids.contains("route:cross-plains-gravel"))
        #expect(!ids.contains { $0.hasPrefix("trip:") })
    }

    @Test
    func deleteTripAndRoutesRemovesTheTripAndItsMembers() {
        let (model, _) = makeModel()

        model.deleteTripAndRoutes(tripID)

        #expect(model.trip(tripID) == nil)
        // The two member routes are gone; the three loose ones remain.
        #expect(model.routes.count == 3)
        let ids = model.plannedItems.map(\.id)
        #expect(!ids.contains("route:devils-lake-overnighter"))
        #expect(!ids.contains("route:cross-plains-gravel"))
        #expect(ids.contains("route:kettle-moraine-loop"))
    }

    // MARK: Trip badge composition (pure)

    @Test
    func tripBadgeIsUpToDateOnlyWhenTripAndEveryStageAre() {
        #expect(
            MainScreenModel.composeTripState(tripSelf: .upToDate, stageStates: [.upToDate, .upToDate])
                == .upToDate)
        #expect(
            MainScreenModel.composeTripState(tripSelf: .upToDate, stageStates: [.upToDate, .outdated])
                == .outdated)
        #expect(
            MainScreenModel.composeTripState(tripSelf: .outdated, stageStates: [.upToDate])
                == .outdated)
        // The trip object itself not on the device → no badge, whatever the stages.
        #expect(
            MainScreenModel.composeTripState(tripSelf: .notOnDevice, stageStates: [.upToDate])
                == .notOnDevice)
        #expect(MainScreenModel.composeTripState(tripSelf: .upToDate, stageStates: []) == .notOnDevice)
    }
}
