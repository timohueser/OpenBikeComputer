import Foundation
import Testing
import OBCDomain
@testable import OBCTransport

/// Trip persistence: a trip round-trips its line, pieces, day ends, stops and device copies through
/// both conformers, and a trip owns its line, so no route change touches it.
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

    private func file(_ lons: [Double], ele: Double? = 500) -> [RoutePoint] {
        lons.map { RoutePoint(coordinate: Coordinate(latitude: 46.5, longitude: $0), elevationMeters: ele, surface: 2) }
    }

    private func trip(_ id: String, addedAt: Date = Date(timeIntervalSince1970: 1_000)) -> Trip {
        Trip.joining(
            [file([8.00, 8.01, 8.02]), file([8.03, 8.04], ele: nil)],
            waypoints: [[Waypoint(
                index: 0, name: "Spring", distanceAlongMeters: 0, coordinate: Coordinate(latitude: 46.5, longitude: 8.01),
                category: .water)]],
            id: TripID(id), name: id, bikeType: .gravel, now: addedAt)
    }

    @Test(arguments: StoreKind.allCases)
    func roundTripsTheWholeTrip(_ kind: StoreKind) {
        let store = makeStore(kind)
        var t = trip("t1")
        let camp = Stop(
            name: "Camp", coordinate: Coordinate(latitude: 46.501, longitude: 8.015), kind: .campsite, mapItemID: "I1")
        let ended = t.endDay(0, at: t.place([camp])[0])
        #expect(ended)
        t.namePlace(1, to: "Brig")
        t.reverse()
        t.renameDay(0, to: "Andermatt")
        t.uploadedKey = 42
        t.startDay = CivilDay(daysSince1970: 20_725)
        let link = DeviceRouteLink(serial: "OBC-001", storeID: "000000000000000000000000a1b2c3d4", objectID: DeviceObjectID(5))
        t.deviceLink = link
        t.uploadedCRC32 = 0xDEAD_BEEF
        t.dayCopies = [nil, TripDayCopy(link: link, uploadedCRC32: 7)]
        store.saveTrip(t)

        #expect(t.startName == "Brig")
        #expect(store.trips() == [t])
    }

    @Test(arguments: StoreKind.allCases)
    func newestFirstAndDelete(_ kind: StoreKind) {
        let store = makeStore(kind)
        store.saveTrip(trip("old", addedAt: Date(timeIntervalSince1970: 1)))
        store.saveTrip(trip("new", addedAt: Date(timeIntervalSince1970: 2)))
        #expect(store.trips().map(\.id.rawValue) == ["new", "old"])

        store.deleteTrip(TripID("new"))
        #expect(store.trips().map(\.id.rawValue) == ["old"])
    }

    @Test(arguments: StoreKind.allCases)
    func deletingARouteLeavesTripsAlone(_ kind: StoreKind) {
        let store = makeStore(kind)
        store.savePlannedRoute(PlannedRouteRecord(
            summary: RouteSummary(id: RouteID("r"), name: "r", distanceMeters: 1, elevationGainMeters: 0),
            route: ImportedRoute(points: file([8.0, 8.1])), sourceFileName: "r.gpx", sourceFileData: Data()))
        store.saveTrip(trip("t"))
        store.deletePlannedRoute(RouteID("r"))
        #expect(store.trips().count == 1)
    }
}
