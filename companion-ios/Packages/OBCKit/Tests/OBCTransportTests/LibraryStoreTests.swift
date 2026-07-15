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
        sourceFileName: String = "Schwarzwald Tour.gpx",
        sourceFileData: Data = Data("<gpx>original import bytes</gpx>".utf8),
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
            sourceFileName: sourceFileName,
            sourceFileData: sourceFileData,
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
        // The basemap coordinates (#294) survive the round-trip — else a
        // relaunched route would silently drop to the grid preview.
        XCTAssertEqual(
            relaunched.first?.summary.trackPreview?.coordinates,
            newer.summary.trackPreview?.coordinates
        )
        XCTAssertFalse(relaunched.first?.summary.trackPreview?.coordinates.isEmpty ?? true)
    }

    func testResaveUpdatesInPlace() {
        let (store, dir) = makeFileStore()
        var record = makeRecord()
        store.savePlannedRoute(record)

        record.summary.name = "Schwarzwald Day 2"   // H12 rename
        // A later upload lands on device object 7 — under the connected
        // device's (serial, epoch) scope (#769).
        record.deviceLink = DeviceRouteLink(serial: "OBC-24-000317", epoch: 42, objectID: DeviceObjectID(7))
        store.savePlannedRoute(record)

        let reloaded = FileLibraryStore(directory: dir).plannedRoutes()
        XCTAssertEqual(reloaded.count, 1)
        XCTAssertEqual(reloaded.first, record)
    }

    func testReplaceImportRewritesSourceSidecar() {
        // A re-import reuses the id, so the sidecar already exists — the new bytes
        // must land, not be silently dropped (the old bug kept the stale file).
        let (store, dir) = makeFileStore()
        store.savePlannedRoute(makeRecord(sourceFileName: "trip.gpx",
                                          sourceFileData: Data("<gpx>v1</gpx>".utf8)))
        store.savePlannedRoute(makeRecord(sourceFileName: "trip.gpx",
                                          sourceFileData: Data("<gpx>v2 replaced</gpx>".utf8)))

        let reloaded = FileLibraryStore(directory: dir).plannedRoutes()
        XCTAssertEqual(reloaded.count, 1)
        XCTAssertEqual(reloaded.first?.sourceFileData, Data("<gpx>v2 replaced</gpx>".utf8))
    }

    func testReplaceImportWithFormatChangeSweepsStaleSidecar() {
        // GPX→TCX changes the sidecar's extension: the new bytes must be readable
        // and the old-extension sidecar swept, so `plannedRoutes()` can't read the
        // stale file (whichever `sourceFileName` names).
        let (store, dir) = makeFileStore()
        store.savePlannedRoute(makeRecord(sourceFileName: "trip.gpx",
                                          sourceFileData: Data("<gpx>from gpx</gpx>".utf8)))
        store.savePlannedRoute(makeRecord(sourceFileName: "trip.tcx",
                                          sourceFileData: Data("<tcx>from tcx</tcx>".utf8)))

        let reloaded = FileLibraryStore(directory: dir).plannedRoutes()
        XCTAssertEqual(reloaded.first?.sourceFileData, Data("<tcx>from tcx</tcx>".utf8))
        // Exactly one sidecar on disk — the stale source.gpx is gone.
        let recordDir = dir.appendingPathComponent("planned/imported-1")
        let sidecars = ((try? FileManager.default.contentsOfDirectory(atPath: recordDir.path)) ?? [])
            .filter { $0.hasPrefix("source.") }
        XCTAssertEqual(sidecars, ["source.tcx"])
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

    // MARK: v1 on-disk compatibility (#359 — no schema bump; #769 — flat links)

    func testV1LibraryFileDecodesWithItsFlatLinkUnclaimed() throws {
        // A `route.json` written by the pre-#359 store (checked in verbatim,
        // generated by that code): the record loads untouched — but its
        // **flat** device link (a bare object id with no serial/epoch, #769)
        // decodes as *no link at all*: it fails the validity predicate by
        // construction, so it can never light a badge or drive a
        // replace-by-id upload against whatever device happens to be
        // connected. V6's CRC adoption is what re-links such records.
        let (store, dir) = makeFileStore()
        let fixture = try XCTUnwrap(Bundle.module.url(
            forResource: "planned-route-v1", withExtension: "json", subdirectory: "Fixtures"))
        let recordDir = dir.appendingPathComponent("planned/imported-v1-fixture", isDirectory: true)
        try FileManager.default.createDirectory(at: recordDir, withIntermediateDirectories: true)
        try FileManager.default.copyItem(at: fixture, to: recordDir.appendingPathComponent("route.json"))

        let loaded = try XCTUnwrap(store.plannedRoutes().first)
        XCTAssertEqual(loaded.id, RouteID("imported-v1-fixture"))
        XCTAssertNil(loaded.deviceLink, "a flat v1 link must not attach to any scope")
        XCTAssertEqual(loaded.uploadedCRC32, 0xDEAD_BEEF)
        XCTAssertEqual(loaded.summary.name, "Schwarzwald Tour")
        XCTAssertEqual(loaded.route.waypoints.count, 1)
        XCTAssertEqual(loaded.addedAt, Date(timeIntervalSince1970: 1_000))
        // Auto-expiry (epic #638) is additive: a pre-expiry file has no retention
        // fields, so all three load `nil` — a `nil` desired level pushes nothing
        // (invariant 6: shipping expiry must not surprise-delete an old route).
        XCTAssertNil(loaded.retention, "a pre-expiry file has no desired retention")
        XCTAssertNil(loaded.deviceExpiresAt)
        XCTAssertNil(loaded.deviceRetention)

        // …and the schema version stays 1 across a re-save (#359's rule: the
        // scope fields are additive, optional-decoded — no bump).
        store.savePlannedRoute(loaded)
        let json = try XCTUnwrap(try JSONSerialization.jsonObject(
            with: Data(contentsOf: recordDir.appendingPathComponent("route.json"))) as? [String: Any])
        XCTAssertEqual(json["version"] as? Int, 1)
    }

    func testScopedLinkRoundTripsThroughDisk() throws {
        // The v2 link persists all three parts and reassembles only when all
        // three are present (#769).
        let (store, dir) = makeFileStore()
        var record = makeRecord()
        record.deviceLink = DeviceRouteLink(
            serial: "OBC-24-000317", epoch: 0xDEAD_0001, objectID: DeviceObjectID(9))
        record.uploadedCRC32 = 0x1234_5678
        store.savePlannedRoute(record)

        let loaded = try XCTUnwrap(FileLibraryStore(directory: dir).plannedRoutes().first)
        XCTAssertEqual(loaded.deviceLink, record.deviceLink)
        XCTAssertEqual(loaded.uploadedCRC32, 0x1234_5678)
        XCTAssertTrue(
            loaded.deviceLink?.matches(LibraryScope(serial: "OBC-24-000317", epoch: 0xDEAD_0001)) == true)
        XCTAssertFalse(
            loaded.deviceLink?.matches(LibraryScope(serial: "OBC-24-000317", epoch: 0xDEAD_0002)) == true,
            "an era change invalidates the link")
    }

    func testRetentionFieldsRoundTripThroughDisk() throws {
        // The auto-expiry fields persist additively (epic #638): a desired level
        // and the device's reported truth survive a save/load, still at schema v1.
        let (store, dir) = makeFileStore()
        var record = makeRecord(id: "with-retention")
        record.retention = .twoWeeks
        record.deviceRetention = .oneMonth
        record.deviceExpiresAt = Date(timeIntervalSince1970: 1_784_808_000)
        store.savePlannedRoute(record)

        let loaded = try XCTUnwrap(
            FileLibraryStore(directory: dir).plannedRoutes().first { $0.id == record.id })
        XCTAssertEqual(loaded.retention, .twoWeeks)
        XCTAssertEqual(loaded.deviceRetention, .oneMonth)
        XCTAssertEqual(loaded.deviceExpiresAt, Date(timeIntervalSince1970: 1_784_808_000))

        // A record with no retention set round-trips as nil (not a defaulted level).
        let plain = makeRecord(id: "no-retention")
        store.savePlannedRoute(plain)
        let reloaded = try XCTUnwrap(
            FileLibraryStore(directory: dir).plannedRoutes().first { $0.id == plain.id })
        XCTAssertNil(reloaded.retention)
    }

    // MARK: Rides + the synced set (H9/H10) — split summary/points (#360)

    func testRideSummariesRoundTripNewestFirst() {
        let (store, dir) = makeFileStore()
        let older = makeRide(id: "ride-a", date: Date(timeIntervalSince1970: 2_000))
        let newer = makeRide(id: "ride-b", name: "Lunch Loop", date: Date(timeIntervalSince1970: 8_000))
        store.saveRide(older)
        store.saveRide(newer)

        // A second instance over the same directory = the app relaunched.
        let summaries = FileLibraryStore(directory: dir).rideSummaries()
        XCTAssertEqual(summaries, [newer.summary, older.summary], "newest first, every field intact")
        XCTAssertEqual(
            summaries.first?.trackPreview?.coordinates,
            newer.summary.trackPreview?.coordinates,
            "the basemap coordinates (#294) survive the round-trip"
        )
    }

    func testRidePointsRoundTripAcrossInstances() {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        store.saveRide(ride)

        XCTAssertEqual(FileLibraryStore(directory: dir).ridePoints(ride.id), ride.points)
        XCTAssertNil(store.ridePoints(RideID("never-synced")), "an unknown id has no tracklog")
    }

    /// The #360 point: listing summaries must not read — let alone decode — the
    /// points files. A deliberately corrupt points file proves it (correctness
    /// beats a flaky timing assert).
    func testRideSummariesNeverDecodeThePointsFiles() {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        store.saveRide(ride)
        try? Data("not json".utf8).write(
            to: dir.appendingPathComponent("rides/ride-1/points.json"))

        XCTAssertEqual(store.rideSummaries(), [ride.summary],
                       "a broken tracklog never costs the list row")
        XCTAssertNil(store.ridePoints(ride.id),
                     "the corrupt points read degrades to summary-only, not a crash")
    }

    /// Loose perf pin, shape not stopwatch: a big library's summaries all load
    /// while **every** points file is unreadable — the only way that passes is
    /// if `rideSummaries()` never touches them.
    func testABigLibraryListsWithoutTouchingAnyPointsFile() {
        let (store, dir) = makeFileStore()
        for index in 0..<200 {
            store.saveRide(makeRide(id: "ride-\(index)",
                                    date: Date(timeIntervalSince1970: Double(index))))
            try? Data("points deliberately unreadable".utf8).write(
                to: dir.appendingPathComponent("rides/ride-\(index)/points.json"))
        }

        XCTAssertEqual(store.rideSummaries().count, 200)
    }

    /// A points file gone missing entirely (half-written v2 dir, manual sweep)
    /// mirrors the undecodable-payload rule: the ride stays a summary-only row
    /// rather than being dropped.
    func testMissingPointsFileKeepsTheSummaryRow() {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        store.saveRide(ride)
        try? FileManager.default.removeItem(
            at: dir.appendingPathComponent("rides/ride-1/points.json"))

        XCTAssertEqual(store.rideSummaries(), [ride.summary])
        XCTAssertNil(store.ridePoints(ride.id))
    }

    /// H12 for rides: a rename persists through the summary-only write — the
    /// tracklog file's bytes stay byte-identical (never re-encoded).
    func testSaveRideSummaryUpdatesTheRowWithoutRewritingPoints() throws {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        store.saveRide(ride)
        let pointsURL = dir.appendingPathComponent("rides/ride-1/points.json")
        let pointBytes = try Data(contentsOf: pointsURL)

        var renamed = ride.summary
        renamed.name = "Dawn Patrol II"
        store.saveRideSummary(renamed)

        XCTAssertEqual(FileLibraryStore(directory: dir).rideSummaries(), [renamed])
        XCTAssertEqual(try Data(contentsOf: pointsURL), pointBytes)
        XCTAssertEqual(store.ridePoints(ride.id), ride.points)
    }

    // MARK: v1 → v2 ride migration (#360)

    /// Copy the checked-in v1 whole-ride file (written verbatim by the pre-#360
    /// store) into a store's `rides/` directory.
    private func installV1RideFixture(in dir: URL) throws {
        let fixture = try XCTUnwrap(Bundle.module.url(
            forResource: "ride-v1", withExtension: "json", subdirectory: "Fixtures"))
        let ridesDir = dir.appendingPathComponent("rides", isDirectory: true)
        try FileManager.default.createDirectory(at: ridesDir, withIntermediateDirectories: true)
        try FileManager.default.copyItem(
            at: fixture, to: ridesDir.appendingPathComponent("ride-v1-fixture.json"))
    }

    func testV1RideFileMigratesOnFirstListRead() throws {
        let (store, dir) = makeFileStore()
        try installV1RideFixture(in: dir)

        // First read: the v1 file loads whole and comes back as a summary…
        let loaded = try XCTUnwrap(store.rideSummaries().first)
        XCTAssertEqual(loaded.id, RideID("ride-v1-fixture"))
        XCTAssertEqual(loaded.name, "Dawn Patrol")
        XCTAssertEqual(loaded.date, Date(timeIntervalSince1970: 2_000))
        XCTAssertEqual(loaded.trackPreview?.coordinates.count, 2)

        // …and the store is rewritten split: old file gone, v2 files in place.
        let rideDir = dir.appendingPathComponent("rides/ride-v1-fixture")
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: dir.appendingPathComponent("rides/ride-v1-fixture.json").path))
        XCTAssertTrue(FileManager.default.fileExists(
            atPath: rideDir.appendingPathComponent("summary.json").path))
        XCTAssertTrue(FileManager.default.fileExists(
            atPath: rideDir.appendingPathComponent("points.json").path))

        // The migrated store survives a second read (= the next launch) with the
        // tracklog intact — elevation-less samples included.
        let relaunched = FileLibraryStore(directory: dir)
        XCTAssertEqual(relaunched.rideSummaries(), [loaded])
        let points = try XCTUnwrap(relaunched.ridePoints(loaded.id))
        XCTAssertEqual(points.count, 2)
        XCTAssertEqual(points.first?.elevationMeters, 300)
        XCTAssertNil(points.last?.elevationMeters)
    }

    /// The detail-before-list order: `ridePoints` on an un-migrated ride splits
    /// the file too — the store never depends on which read comes first.
    func testV1RideFileMigratesOnAPointsRead() throws {
        let (store, dir) = makeFileStore()
        try installV1RideFixture(in: dir)

        let points = try XCTUnwrap(store.ridePoints(RideID("ride-v1-fixture")))
        XCTAssertEqual(points.count, 2)
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: dir.appendingPathComponent("rides/ride-v1-fixture.json").path))
        XCTAssertEqual(store.rideSummaries().count, 1)
    }

    /// A rename must migrate a lingering v1 file *first* — otherwise the next
    /// lazy migration would rewrite the summary from the stale whole-ride file
    /// and silently undo the rename.
    func testRenameOfAnUnmigratedRideSurvivesTheNextRead() throws {
        let (store, dir) = makeFileStore()
        try installV1RideFixture(in: dir)

        // No list read first: the summary-only write itself finds the v1 file.
        let renamed = RideSummary(
            id: RideID("ride-v1-fixture"), name: "Renamed After Migration",
            date: Date(timeIntervalSince1970: 2_000),
            distanceMeters: 31_000, movingTime: 4_500,
            averageSpeedMps: 6.9, climbMeters: 410
        )
        store.saveRideSummary(renamed)

        let relaunched = FileLibraryStore(directory: dir)
        XCTAssertEqual(relaunched.rideSummaries().first?.name, "Renamed After Migration")
        XCTAssertEqual(relaunched.ridePoints(renamed.id)?.count, 2, "the tracklog rode along")
    }

    func testSyncedIDsSurviveRideDeleteAndRelaunch() {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        store.saveRide(ride)
        store.markRideSynced(ride.id)
        store.deleteRide(ride.id)

        let relaunched = FileLibraryStore(directory: dir)
        XCTAssertTrue(relaunched.rideSummaries().isEmpty)
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

    func testTrashedRideMarksSurviveRelaunchAndKeepTheFiles() {
        let (store, dir) = makeFileStore()
        let kept = makeRide()
        let recovered = makeRide(id: "ride-2", name: "Second")
        store.saveRide(kept)
        store.saveRide(recovered)
        let trashedAt = Date(timeIntervalSince1970: 1_700_000_000)
        store.markRideTrashed(kept.id, at: trashedAt)
        store.markRideTrashed(recovered.id, at: trashedAt.addingTimeInterval(60))
        store.unmarkRideTrashed(recovered.id)

        let relaunched = FileLibraryStore(directory: dir)
        XCTAssertEqual(relaunched.trashedRideIDs(), [kept.id: trashedAt])
        // Trash is a mark, not a move — the stored files stay readable, which
        // is what makes Recover instant (#292).
        XCTAssertEqual(Set(relaunched.rideSummaries().map(\.id)), [kept.id, recovered.id])
        XCTAssertEqual(relaunched.ridePoints(kept.id), kept.points)
    }

    func testAwkwardIDsStayDistinctOnDisk() {
        // Device ride ids are firmware-owned strings — path separators and
        // near-collisions must not merge records.
        let (store, dir) = makeFileStore()
        let a = makeRide(id: "rides/2026-07-01 08:12")
        let b = makeRide(id: "rides_2026-07-01 08:12", name: "Twin")
        store.saveRide(a)
        store.saveRide(b)

        let reloaded = FileLibraryStore(directory: dir).rideSummaries()
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
        XCTAssertEqual(store.rideSummaries(), [ride.summary])
        XCTAssertEqual(store.ridePoints(ride.id), ride.points)

        var renamed = ride.summary
        renamed.name = "Dawn Patrol II"
        store.saveRideSummary(renamed)
        XCTAssertEqual(store.rideSummaries(), [renamed])
        XCTAssertEqual(store.ridePoints(ride.id), ride.points, "a rename never touches points")

        let trashedAt = Date(timeIntervalSince1970: 1_700_000_000)
        store.markRideTrashed(ride.id, at: trashedAt)
        XCTAssertEqual(store.trashedRideIDs(), [ride.id: trashedAt])
        XCTAssertEqual(store.rideSummaries(), [renamed], "trash is a mark, not a move")
        store.unmarkRideTrashed(ride.id)
        XCTAssertTrue(store.trashedRideIDs().isEmpty)

        store.deletePlannedRoute(record.id)
        store.markRideDeleted(ride.id)
        store.deleteRide(ride.id)
        XCTAssertTrue(store.plannedRoutes().isEmpty)
        XCTAssertTrue(store.rideSummaries().isEmpty)
        XCTAssertNil(store.ridePoints(ride.id), "the tracklog dies with the ride")
        XCTAssertEqual(store.syncedRideIDs(), [ride.id], "the synced marker survives the delete")
        XCTAssertEqual(store.deletedRideIDs(), [ride.id], "the tombstone survives the delete")
    }
}
