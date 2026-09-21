import Foundation
import Testing
import OBCDomain
@testable import OBCTransport

/// Trip persistence: trips round-trip through both conformers, a route stays in at most one trip
/// on every `saveTrip`, reads drop dangling stages, `deleteTrip` ungroups without touching the
/// routes, and deleting a planned route prunes it. Every case runs against both store kinds.
struct TripLibraryStoreTests {
    enum StoreKind: CaseIterable { case inMemory, file }

    private func makeStore(_ kind: StoreKind) -> LibraryStore {
        switch kind {
        case .inMemory:
            return InMemoryLibraryStore()
        case .file:
            let dir = URL(fileURLWithPath: NSTemporaryDirectory())
                .appendingPathComponent("obc-trip-tests-\(UUID().uuidString)", isDirectory: true)
            return FileLibraryStore(directory: dir)
        }
    }

    private func trip(_ id: String, _ stages: [String], name: String? = nil,
                      addedAt: Date = Date(timeIntervalSince1970: 1_000)) -> TripRecord {
        TripRecord(id: TripID(id), name: name ?? id, stageIDs: stages.map(RouteID.init), addedAt: addedAt)
    }

    /// A minimal planned record so a stage id resolves: reads keep only stages whose route
    /// record exists.
    private func plannedRoute(_ id: String) -> PlannedRouteRecord {
        PlannedRouteRecord(
            summary: RouteSummary(id: RouteID(id), name: id, distanceMeters: 1_000, elevationGainMeters: 100),
            route: ImportedRoute(points: []),
            sourceFileName: "\(id).gpx",
            sourceFileData: Data()
        )
    }

    @Test(arguments: StoreKind.allCases)
    func savesAndReadsBackInRideOrder(_ kind: StoreKind) {
        let store = makeStore(kind)
        ["a", "b", "c"].forEach { store.savePlannedRoute(plannedRoute($0)) }
        var t = trip("t1", ["c", "a", "b"], name: "Alpen Traverse")
        let link = DeviceRouteLink(serial: "OBC-001", storeID: "000000000000000000000000a1b2c3d4", objectID: DeviceObjectID(5))
        t.deviceLink = link
        t.uploadedCRC32 = 0xDEAD_BEEF
        store.saveTrip(t)

        let got = store.trips()
        #expect(got.count == 1)
        #expect(got[0].id == TripID("t1"))
        #expect(got[0].name == "Alpen Traverse")
        #expect(got[0].stageIDs == [RouteID("c"), RouteID("a"), RouteID("b")])  // order preserved
        #expect(got[0].deviceLink == link)
        #expect(got[0].uploadedCRC32 == 0xDEAD_BEEF)
    }

    @Test(arguments: StoreKind.allCases)
    func newestFirstOrder(_ kind: StoreKind) {
        let store = makeStore(kind)
        ["a", "b"].forEach { store.savePlannedRoute(plannedRoute($0)) }
        store.saveTrip(trip("old", ["a"], addedAt: Date(timeIntervalSince1970: 1_000)))
        store.saveTrip(trip("new", ["b"], addedAt: Date(timeIntervalSince1970: 2_000)))
        #expect(store.trips().map(\.id) == [TripID("new"), TripID("old")])
    }

    @Test(arguments: StoreKind.allCases)
    func savingATripStealsSharedStagesFromOtherTrips(_ kind: StoreKind) {
        let store = makeStore(kind)
        ["r1", "r2", "r3"].forEach { store.savePlannedRoute(plannedRoute($0)) }
        store.saveTrip(trip("A", ["r1", "r2"]))
        // B claims r2, so it must leave A: a route lives in at most one trip.
        store.saveTrip(trip("B", ["r2", "r3"]))

        let byID = Dictionary(uniqueKeysWithValues: store.trips().map { ($0.id, $0.stageIDs) })
        #expect(byID[TripID("A")] == [RouteID("r1")])
        #expect(byID[TripID("B")] == [RouteID("r2"), RouteID("r3")])
    }

    @Test(arguments: StoreKind.allCases)
    func aTripEmptiedByAStealDissolves(_ kind: StoreKind) {
        let store = makeStore(kind)
        ["r1"].forEach { store.savePlannedRoute(plannedRoute($0)) }
        store.saveTrip(trip("A", ["r1"]))
        store.saveTrip(trip("B", ["r1"]))  // steals A's only stage
        let trips = store.trips()
        #expect(trips.map(\.id) == [TripID("B")])
        #expect(trips[0].stageIDs == [RouteID("r1")])
    }

    @Test(arguments: StoreKind.allCases)
    func readsDropStagesWhoseRouteRecordIsGone(_ kind: StoreKind) {
        let store = makeStore(kind)
        store.savePlannedRoute(plannedRoute("r1"))  // r2 never exists
        store.saveTrip(trip("A", ["r1", "r2"]))
        #expect(store.trips().first?.stageIDs == [RouteID("r1")])  // r2 pruned on read

        // A trip with no resolvable stage at all is dropped entirely.
        store.saveTrip(trip("B", ["ghost"]))
        #expect(store.trips().map(\.id) == [TripID("A")])
    }

    @Test(arguments: StoreKind.allCases)
    func deleteTripUngroupsButLeavesRoutes(_ kind: StoreKind) {
        let store = makeStore(kind)
        ["r1", "r2"].forEach { store.savePlannedRoute(plannedRoute($0)) }
        store.saveTrip(trip("A", ["r1", "r2"]))
        store.deleteTrip(TripID("A"))
        #expect(store.trips().isEmpty)
        #expect(Set(store.plannedRoutes().map(\.id)) == [RouteID("r1"), RouteID("r2")])
    }

    @Test(arguments: StoreKind.allCases)
    func deletePlannedRoutePrunesItFromTrips(_ kind: StoreKind) {
        let store = makeStore(kind)
        ["r1", "r2"].forEach { store.savePlannedRoute(plannedRoute($0)) }
        store.saveTrip(trip("A", ["r1", "r2"]))
        store.deletePlannedRoute(RouteID("r1"))
        #expect(store.trips().first?.stageIDs == [RouteID("r2")])

        // Deleting the last member dissolves the trip.
        store.deletePlannedRoute(RouteID("r2"))
        #expect(store.trips().isEmpty)
    }

    @Test(arguments: StoreKind.allCases)
    func aPreTripsLibraryLoadsWithZeroTrips(_ kind: StoreKind) {
        let store = makeStore(kind)
        store.savePlannedRoute(plannedRoute("r1"))  // routes but never any trip
        #expect(store.trips().isEmpty)  // additive schema, no migration
    }

    @Test
    func fileStorePersistsAcrossInstances() throws {
        let dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("obc-trip-persist-\(UUID().uuidString)", isDirectory: true)
        var t = trip("t1", ["r1", "r2"], name: "Persisted")
        let link = DeviceRouteLink(serial: "OBC-042", storeID: "1111111111111111111111110badf00d", objectID: DeviceObjectID(9))
        t.deviceLink = link
        t.uploadedCRC32 = 0x1234_5678
        do {
            let store = FileLibraryStore(directory: dir)
            ["r1", "r2"].forEach { store.savePlannedRoute(plannedRoute($0)) }
            store.saveTrip(t)
        }
        // A fresh instance is an app relaunch; the scoped link survives whole.
        let reopened = FileLibraryStore(directory: dir)
        let got = reopened.trips()
        #expect(got.count == 1)
        #expect(got[0].name == "Persisted")
        #expect(got[0].stageIDs == [RouteID("r1"), RouteID("r2")])
        #expect(got[0].deviceLink == link)
        #expect(got[0].uploadedCRC32 == 0x1234_5678)
        let file = dir.appendingPathComponent("trips/t1.json")
        var json = try #require(JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
        json.removeValue(forKey: "deviceStoreID")
        json["deviceStoreEpoch"] = 0x0BAD_F00D
        try JSONSerialization.data(withJSONObject: json).write(to: file)
        #expect(reopened.trips().first?.deviceLink == nil)

    }

    @Test
    func aPartialOnDiskLinkDecodesAsNoLink() throws {
        // All or nothing: a trip file carrying a bare object id must load with no device link at
        // all, so it can never light a badge or drive a replace-by-id against the wrong device.
        let dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("obc-trip-flat-\(UUID().uuidString)", isDirectory: true)
        let store = FileLibraryStore(directory: dir)
        store.savePlannedRoute(plannedRoute("r1"))
        store.saveTrip(trip("t1", ["r1"]))

        // Rewrite the stored file with the id alone (a hand-rolled flat link).
        let url = dir.appendingPathComponent("trips/t1.json")
        var json = try #require(
            try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        json["deviceObjectID"] = 9
        json.removeValue(forKey: "deviceSerial")
        json.removeValue(forKey: "deviceStoreEpoch")
        try JSONSerialization.data(withJSONObject: json).write(to: url)

        let got = store.trips()
        #expect(got.count == 1)
        #expect(got[0].deviceLink == nil)
    }
}

/// Trip domain helpers: the one derived-stats implementation and the trip-level `OnDeviceState`
/// reuse.
struct TripDomainTests {
    private func summary(_ id: String, distance: Double, ascent: Double) -> RouteSummary {
        RouteSummary(id: RouteID(id), name: id, distanceMeters: distance, elevationGainMeters: ascent)
    }

    @Test
    func statsSumDistanceAscentAndStageCount() {
        let stats = TripStats.summing([
            summary("a", distance: 1_000, ascent: 100),
            summary("b", distance: 2_500, ascent: 250),
        ])
        #expect(stats.distanceMeters == 3_500)
        #expect(stats.elevationGainMeters == 350)
        #expect(stats.stageCount == 2)
    }

    @Test
    func emptyTripStatsAreZero() {
        #expect(TripStats.summing([]) == TripStats(distanceMeters: 0, elevationGainMeters: 0, stageCount: 0))
    }

    @Test
    func onDeviceStateReusesTheRouteRule() {
        let trip = TripRecord(id: TripID("t"), name: "T", stageIDs: [RouteID("a")])
        // No proven CRC means not on device: never a badge without proof.
        #expect(trip.onDeviceState(provenCommittedCRC: nil, currentCRC: { 0x1 }) == .notOnDevice)
        #expect(trip.onDeviceState(provenCommittedCRC: 0xABCD, currentCRC: { 0xABCD }) == .upToDate)
        #expect(trip.onDeviceState(provenCommittedCRC: 0xABCD, currentCRC: { 0x1 }) == .outdated)
    }
}
