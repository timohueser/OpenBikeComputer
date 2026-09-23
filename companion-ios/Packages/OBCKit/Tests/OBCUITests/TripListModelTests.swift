import Testing
import Foundation
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The Planned list with trips: the interleave, trip stats from the day routes, delete, and the
/// trip badge composition. The edits run through `MainScreenModel` on the `trips` fixture.
@MainActor
struct TripListModelTests {
    private let tripID = TripID("driftless-weekender")

    private func makeModel() -> MainScreenModel {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.loadFixtures("trips")
        let library = InMemoryLibraryStore()
        control.seedLibrary(into: library)
        let model = MainScreenModel(transport: MockTransport(control: control), library: library)
        model.start()  // library-first content (trips + items) is set synchronously
        return model
    }

    @Test
    func partitionInterleavesTripsAndRoutesByAddedAt() {
        let base = Date()
        func record(_ id: String, addedAt: Date) -> PlannedRouteRecord {
            PlannedRouteRecord(
                summary: RouteSummary(id: RouteID(id), name: id, distanceMeters: 1_000, elevationGainMeters: 100),
                route: ImportedRoute(points: []),
                sourceFileName: "\(id).gpx", sourceFileData: Data(), addedAt: addedAt)
        }
        let records = [record("new", addedAt: base), record("old", addedAt: base.addingTimeInterval(-30))]
        let trip = Trip(id: TripID("t"), name: "Trip", bikeType: .road, addedAt: base.addingTimeInterval(-15))

        let items = PlannedItem.partition(records: records, trips: [trip])

        #expect(items.map(\.id) == ["route:new", "trip:t", "route:old"])
    }

    @Test
    func tripStatsSumTheDayRoutes() {
        let model = makeModel()
        let days = model.tripDays(tripID)
        let stats = model.tripStats(tripID)
        #expect(days.map(\.name) == ["Day 1", "Day 2"])
        #expect(stats.dayCount == 2)
        #expect(stats.distanceMeters == days.reduce(0) { $0 + $1.distanceMeters })
        #expect(stats.distanceMeters > 0)
    }

    @Test
    func deleteTripRemovesItFromTheList() {
        let model = makeModel()
        model.deleteTrip(tripID)
        #expect(model.trip(tripID) == nil)
        #expect(!model.plannedItems.map(\.id).contains { $0.hasPrefix("trip:") })
        #expect(model.routes.count == 3)
    }

    @Test
    func tripBadgeIsUpToDateOnlyWhenTripAndEveryDayAre() {
        #expect(
            MainScreenModel.composeTripState(tripSelf: .upToDate, dayStates: [.upToDate, .upToDate])
                == .upToDate)
        #expect(
            MainScreenModel.composeTripState(tripSelf: .upToDate, dayStates: [.upToDate, .outdated])
                == .outdated)
        #expect(
            MainScreenModel.composeTripState(tripSelf: .outdated, dayStates: [.upToDate])
                == .outdated)
        // A trip object not on the device gets no badge, whatever the days say.
        #expect(
            MainScreenModel.composeTripState(tripSelf: .notOnDevice, dayStates: [.upToDate])
                == .notOnDevice)
        #expect(MainScreenModel.composeTripState(tripSelf: .upToDate, dayStates: []) == .notOnDevice)
    }
}
