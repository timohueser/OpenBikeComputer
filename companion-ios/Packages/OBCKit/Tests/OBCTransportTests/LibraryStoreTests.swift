import XCTest
import OBCDomain
import OBCTransport

/// B1S acceptance, store-side: the file-backed library round-trips the
/// canonical models across instances (= app relaunches), keeps the original
/// import bytes byte-exact, and the synced-ride set survives both a ride
/// delete and a relaunch (idempotent re-sync, H9).
final class LibraryStoreTests: XCTestCase {
    // MARK: Fixtures

    private func makeRecord(
        id: String = "imported-1",
        name: String = "Schwarzwald Tour",
        addedAt: Date = Date(timeIntervalSince1970: 1_000)
    ) -> PlannedRouteRecord {
        let points = [
            RoutePoint(coordinate: Coordinate(latitude: 48.0, longitude: 8.0), elevationMeters: 500),
            RoutePoint(coordinate: Coordinate(latitude: 48.1, longitude: 8.1), elevationMeters: 620),
            RoutePoint(coordinate: Coordinate(latitude: 48.2, longitude: 8.05)),
        ]
        let route = ImportedRoute(
            name: name, creator: "komoot", points: points,
            waypoints: [
                Waypoint(index: 0, name: "Bakery", note: "coffee", distanceAlongMeters: 1_200,
                         coordinate: Coordinate(latitude: 48.05, longitude: 8.02))
            ]
        )
        let summary = RouteSummary(
            id: RouteID(id), name: name,
            distanceMeters: 24_000, elevationGainMeters: 800,
            estimatedDuration: 5_400, pointCount: points.count, source: .gpx,
            trackPreview: TrackPreview.normalizing(points.map(\.coordinate))
        )
        return PlannedRouteRecord(
            summary: summary, route: route,
            sourceFileName: "Schwarzwald Tour.gpx",
            sourceFileData: Data("<gpx>original import bytes</gpx>".utf8),
            addedAt: addedAt
        )
    }

    private func makeRide(
        id: String = "ride-1",
        name: String = "Dawn Patrol",
        date: Date = Date(timeIntervalSince1970: 2_000)
    ) -> Ride {
        Ride(
            summary: RideSummary(
                id: RideID(id), name: name, date: date,
                distanceMeters: 31_000, movingTime: 4_500,
                averageSpeedMps: 6.9, climbMeters: 410,
                trackPreview: TrackPreview.normalizing([
                    Coordinate(latitude: 47.0, longitude: 7.0),
                    Coordinate(latitude: 47.1, longitude: 7.2),
                ])
            ),
            points: [
                RidePoint(timestamp: Date(timeIntervalSince1970: 2_000),
                          coordinate: Coordinate(latitude: 47.0, longitude: 7.0), elevationMeters: 300),
                RidePoint(timestamp: Date(timeIntervalSince1970: 2_060),
                          coordinate: Coordinate(latitude: 47.1, longitude: 7.2)),
            ]
        )
    }

    /// A fresh on-disk store in its own temp directory, cleaned up after the test.
    private func makeFileStore() -> (store: FileLibraryStore, directory: URL) {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("obc-library-tests-\(UUID().uuidString)", isDirectory: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: dir) }
        return (FileLibraryStore(directory: dir), dir)
    }

    // MARK: Planned routes — the H4 "survives relaunch" core

    func testPlannedRouteRoundTripsAcrossInstances() {
        let (store, dir) = makeFileStore()
        let older = makeRecord(id: "imported-a", addedAt: Date(timeIntervalSince1970: 1_000))
        let newer = makeRecord(id: "imported-b", name: "Vosges Crossing",
                               addedAt: Date(timeIntervalSince1970: 9_000))
        store.savePlannedRoute(older)
        store.savePlannedRoute(newer)

        // A second instance over the same directory = the app relaunched.
        let relaunched = FileLibraryStore(directory: dir).plannedRoutes()
        XCTAssertEqual(relaunched, [newer, older], "newest first, every field intact")
        XCTAssertEqual(relaunched.first?.sourceFileData, newer.sourceFileData, "original bytes byte-exact")
    }

    func testResaveUpdatesInPlace() {
        let (store, dir) = makeFileStore()
        var record = makeRecord()
        store.savePlannedRoute(record)

        record.summary.name = "Schwarzwald Day 2"   // H12 rename
        record.deviceObjectID = 7                    // a later upload lands on device object 7
        store.savePlannedRoute(record)

        let reloaded = FileLibraryStore(directory: dir).plannedRoutes()
        XCTAssertEqual(reloaded.count, 1)
        XCTAssertEqual(reloaded.first, record)
    }

    func testDeletePlannedRouteRemovesRecordAndSourceFile() {
        let (store, dir) = makeFileStore()
        let record = makeRecord()
        store.savePlannedRoute(record)
        store.deletePlannedRoute(record.id)

        XCTAssertTrue(store.plannedRoutes().isEmpty)
        // Nothing left behind on disk either — the source sidecar goes with it.
        let planned = dir.appendingPathComponent("planned")
        let leftovers = (try? FileManager.default.contentsOfDirectory(atPath: planned.path)) ?? []
        XCTAssertTrue(leftovers.isEmpty)
    }

    func testUnreadableRecordIsSkippedNotFatal() {
        let (store, dir) = makeFileStore()
        store.savePlannedRoute(makeRecord())
        let rogue = dir.appendingPathComponent("planned/rogue", isDirectory: true)
        try? FileManager.default.createDirectory(at: rogue, withIntermediateDirectories: true)
        try? Data("not json".utf8).write(to: rogue.appendingPathComponent("route.json"))

        XCTAssertEqual(FileLibraryStore(directory: dir).plannedRoutes().count, 1)
    }

    // MARK: Rides + the synced set (H9/H10)

    func testRidesRoundTripNewestFirst() {
        let (store, dir) = makeFileStore()
        let older = makeRide(id: "ride-a", date: Date(timeIntervalSince1970: 2_000))
        let newer = makeRide(id: "ride-b", name: "Lunch Loop", date: Date(timeIntervalSince1970: 8_000))
        store.saveRide(older)
        store.saveRide(newer)

        XCTAssertEqual(FileLibraryStore(directory: dir).rides(), [newer, older])
    }

    func testSyncedIDsSurviveRideDeleteAndRelaunch() {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        store.saveRide(ride)
        store.markRideSynced(ride.id)
        store.deleteRide(ride.id)

        let relaunched = FileLibraryStore(directory: dir)
        XCTAssertTrue(relaunched.rides().isEmpty)
        // The idempotence marker outlives the ride — a deleted ride must not
        // come back as "new" on the next sync.
        XCTAssertEqual(relaunched.syncedRideIDs(), [ride.id])
    }

    func testDeletedTombstonesSurviveRelaunch() {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        store.saveRide(ride)
        store.markRideDeleted(ride.id)
        store.deleteRide(ride.id)

        // The tombstone is what keeps the device's copy (still on its SD card)
        // out of the merged list after a relaunch.
        XCTAssertEqual(FileLibraryStore(directory: dir).deletedRideIDs(), [ride.id])
    }

    func testAwkwardIDsStayDistinctOnDisk() {
        // Device ride ids are firmware-owned strings — path separators and
        // near-collisions must not merge records.
        let (store, dir) = makeFileStore()
        let a = makeRide(id: "rides/2026-07-01 08:12")
        let b = makeRide(id: "rides_2026-07-01 08:12", name: "Twin")
        store.saveRide(a)
        store.saveRide(b)

        let reloaded = FileLibraryStore(directory: dir).rides()
        XCTAssertEqual(Set(reloaded.map(\.id)), [a.id, b.id])
    }

    // MARK: The in-memory conformer (previews / mock runs)

    func testInMemoryStoreBehavesLikeALibrary() {
        let store = InMemoryLibraryStore()
        let record = makeRecord()
        let ride = makeRide()

        store.savePlannedRoute(record)
        store.saveRide(ride)
        store.markRideSynced(ride.id)
        XCTAssertEqual(store.plannedRoutes(), [record])
        XCTAssertEqual(store.rides(), [ride])

        store.deletePlannedRoute(record.id)
        store.markRideDeleted(ride.id)
        store.deleteRide(ride.id)
        XCTAssertTrue(store.plannedRoutes().isEmpty)
        XCTAssertTrue(store.rides().isEmpty)
        XCTAssertEqual(store.syncedRideIDs(), [ride.id], "the synced marker survives the delete")
        XCTAssertEqual(store.deletedRideIDs(), [ride.id], "the tombstone survives the delete")
    }
}
