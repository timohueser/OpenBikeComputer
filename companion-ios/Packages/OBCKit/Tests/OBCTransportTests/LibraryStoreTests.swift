import XCTest
import OBCDomain
import OBCTransport

/// The file-backed library store: the canonical models round-trip across instances (that is,
/// app relaunches), import bytes stay byte-exact, and the synced-ride set survives a delete.
final class LibraryStoreTests: XCTestCase {
    private func pointsURL(in directory: URL, id: String) throws -> URL {
        let ride = directory.appendingPathComponent("rides/\(id)")
        let data = try Data(contentsOf: ride.appendingPathComponent("summary.json"))
        let manifest = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        return ride.appendingPathComponent(try XCTUnwrap(manifest["pointsFile"] as? String))
    }

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
            RoutePoint(coordinate: Coordinate(latitude: 48.1, longitude: 8.1), elevationMeters: 620, surface: 3, elevationIncomplete: true),
            RoutePoint(coordinate: Coordinate(latitude: 48.2, longitude: 8.05)),
        ]
        let route = ImportedRoute(
            name: name, creator: "komoot", points: points,
            waypoints: [
                Waypoint(index: 0, name: "Bakery", note: "coffee", distanceAlongMeters: 1_200,
                         coordinate: Coordinate(latitude: 48.05, longitude: 8.02),
                         provenance: WaypointProvenance(store: Data(repeating: 1, count: 16), object: 2, revision: 3, ordinal: 4))
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
                ]),
                bikeType: .touring,
                trip: RideTrip(key: 42, dayIndex: 1, dayCount: 3, name: "Alpen Traverse")
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

    // MARK: Planned routes

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
        // Without the preview coordinates a relaunched route drops to the grid preview.
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

        record.summary.name = "Schwarzwald Day 2"
        // A later upload lands on device object 7, under the connected device's scope.
        record.deviceLink = DeviceRouteLink(serial: "OBC-24-000317", storeID: "0000000000000000000000000000002a", objectID: DeviceObjectID(7))
        store.savePlannedRoute(record)

        let reloaded = FileLibraryStore(directory: dir).plannedRoutes()
        XCTAssertEqual(reloaded.count, 1)
        XCTAssertEqual(reloaded.first, record)
    }

    func testReplaceImportRewritesSourceSidecar() {
        // A re-import reuses the id, so the sidecar already exists: the new bytes must replace it.
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
        // A format change moves the sidecar's extension. The old-extension file must be swept,
        // or `plannedRoutes()` can read the stale one.
        let (store, dir) = makeFileStore()
        store.savePlannedRoute(makeRecord(sourceFileName: "trip.gpx",
                                          sourceFileData: Data("<gpx>from gpx</gpx>".utf8)))
        store.savePlannedRoute(makeRecord(sourceFileName: "trip.tcx",
                                          sourceFileData: Data("<tcx>from tcx</tcx>".utf8)))

        let reloaded = FileLibraryStore(directory: dir).plannedRoutes()
        XCTAssertEqual(reloaded.first?.sourceFileData, Data("<tcx>from tcx</tcx>".utf8))
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

    // MARK: v1 on-disk compatibility

    func testV1LibraryFileDecodesWithItsFlatLinkUnclaimed() throws {
        // A `route.json` fixture with no scope fields: the record loads untouched, but its flat
        // device link (a bare object id) decodes as no link at all. It can never light a badge or
        // drive a replace-by-id upload against whatever device happens to be connected.
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

        // The schema version stays 1 across a re-save: the scope fields are additive and optional.
        store.savePlannedRoute(loaded)
        let json = try XCTUnwrap(try JSONSerialization.jsonObject(
            with: Data(contentsOf: recordDir.appendingPathComponent("route.json"))) as? [String: Any])
        XCTAssertEqual(json["version"] as? Int, 1)
    }

    func testScopedLinkRoundTripsThroughDisk() throws {
        let (store, dir) = makeFileStore()
        var record = makeRecord()
        record.deviceLink = DeviceRouteLink(
            serial: "OBC-24-000317", storeID: "111111111111111111111111dead0001", objectID: DeviceObjectID(9))
        record.uploadedCRC32 = 0x1234_5678
        store.savePlannedRoute(record)

        let loaded = try XCTUnwrap(FileLibraryStore(directory: dir).plannedRoutes().first)
        XCTAssertEqual(loaded.deviceLink, record.deviceLink)
        XCTAssertEqual(loaded.uploadedCRC32, 0x1234_5678)
        XCTAssertTrue(
            loaded.deviceLink?.matches(LibraryScope(serial: "OBC-24-000317", storeID: "111111111111111111111111dead0001")) == true)
        XCTAssertFalse(
            loaded.deviceLink?.matches(LibraryScope(serial: "OBC-24-000317", storeID: "222222222222222222222222dead0001")) == true,
            "an era change invalidates the link")
        let file = dir.appendingPathComponent("planned/imported-1/route.json")
        var json = try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
        json.removeValue(forKey: "deviceStoreID")
        json["deviceStoreEpoch"] = 0xDEAD_0001
        try JSONSerialization.data(withJSONObject: json).write(to: file)
        XCTAssertNil(store.plannedRoutes().first?.deviceLink)

    }
    // MARK: Rides and the synced set

    func testRideSummariesRoundTripNewestFirst() throws {
        let (store, dir) = makeFileStore()
        let older = makeRide(id: "ride-a", date: Date(timeIntervalSince1970: 2_000))
        let newer = makeRide(id: "ride-b", name: "Lunch Loop", date: Date(timeIntervalSince1970: 8_000))
        try store.saveRide(older)
        try store.saveRide(newer)

        let summaries = FileLibraryStore(directory: dir).rideSummaries()
        XCTAssertEqual(summaries, [newer.summary, older.summary], "newest first, every field intact")
        XCTAssertEqual(
            summaries.first?.trackPreview?.coordinates,
            newer.summary.trackPreview?.coordinates,
            "the basemap coordinates (#294) survive the round-trip"
        )
    }

    func testRidePointsRoundTripAcrossInstances() throws {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        try store.saveRide(ride)

        XCTAssertEqual(FileLibraryStore(directory: dir).ridePoints(ride.id), ride.points)
        XCTAssertNil(store.ridePoints(RideID("never-synced")), "an unknown id has no tracklog")
    }

    /// Listing summaries must not read, let alone decode, the points files. A deliberately
    /// corrupt points file proves it without a flaky timing assert.
    func testRideSummariesNeverDecodeThePointsFiles() throws {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        try store.saveRide(ride)
        try? Data("not json".utf8).write(
            to: try pointsURL(in: dir, id: "ride-1"))

        XCTAssertEqual(store.rideSummaries(), [ride.summary],
                       "a broken tracklog never costs the list row")
        XCTAssertNil(store.ridePoints(ride.id),
                     "the corrupt points read degrades to summary-only, not a crash")
    }

    /// A big library lists while every points file is unreadable: the only way that passes is if
    /// `rideSummaries()` never touches them.
    func testABigLibraryListsWithoutTouchingAnyPointsFile() throws {
        let (store, dir) = makeFileStore()
        for index in 0..<200 {
            try store.saveRide(makeRide(id: "ride-\(index)",
                                    date: Date(timeIntervalSince1970: Double(index))))
            try? Data("points deliberately unreadable".utf8).write(
                to: try pointsURL(in: dir, id: "ride-\(index)"))
        }

        XCTAssertEqual(store.rideSummaries().count, 200)
    }

    /// The map line is built once: a later read needs no points file, and a re-saved ride
    /// rebuilds it from the new points.
    func testRideMapLineIsCachedUntilThePointsChange() throws {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        try store.saveRide(ride)
        let line = try XCTUnwrap(store.rideMapLine(ride.id))
        XCTAssertEqual(line.pieces, [ride.points.map(\.coordinate)])

        try Data("not json".utf8).write(to: try pointsURL(in: dir, id: "ride-1"))
        XCTAssertEqual(FileLibraryStore(directory: dir).rideMapLine(ride.id), line)

        let moved = Ride(summary: ride.summary, points: [ride.points[0]] + [
            RidePoint(timestamp: Date(timeIntervalSince1970: 2_060), coordinate: Coordinate(latitude: 47.2, longitude: 7.3)),
        ])
        try store.saveRide(moved)
        XCTAssertEqual(store.rideMapLine(ride.id)?.pieces, [moved.points.map(\.coordinate)])

        store.deleteRide(ride.id)
        XCTAssertNil(store.rideMapLine(ride.id))
        XCTAssertFalse(FileManager.default.fileExists(atPath: dir.appendingPathComponent("ride-lines/ride-1.json").path))
    }

    /// A line cached by another build of the simplifier rebuilds from the points.
    func testRideMapLineFromAnotherFormatRebuilds() throws {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        try store.saveRide(ride)
        _ = store.rideMapLine(ride.id)
        let url = dir.appendingPathComponent("ride-lines/ride-1.json")
        var cached = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        cached["version"] = RideMapLine.formatVersion + 1
        cached["pieces"] = [[0.0, 0.0, 1.0, 1.0]]
        try JSONSerialization.data(withJSONObject: cached).write(to: url)

        XCTAssertEqual(store.rideMapLine(ride.id)?.pieces, [ride.points.map(\.coordinate)])
    }

    func testMissingPointsFileKeepsTheSummaryRow() throws {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        try store.saveRide(ride)
        try? FileManager.default.removeItem(
            at: try pointsURL(in: dir, id: "ride-1"))

        XCTAssertEqual(store.rideSummaries(), [ride.summary])
        XCTAssertNil(store.ridePoints(ride.id))
    }

    func testSaveRideSummaryUpdatesTheRowWithoutRewritingPoints() throws {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        try store.saveRide(ride)
        let pointsURL = try pointsURL(in: dir, id: "ride-1")
        let pointBytes = try Data(contentsOf: pointsURL)

        var renamed = ride.summary
        renamed.name = "Dawn Patrol II"
        store.saveRideSummary(renamed)

        XCTAssertEqual(FileLibraryStore(directory: dir).rideSummaries(), [renamed])
        XCTAssertEqual(try Data(contentsOf: pointsURL), pointBytes)
        XCTAssertEqual(store.ridePoints(ride.id), ride.points)
    }

    func testUnsupportedWholeRideFileSurvivesCurrentRideOperations() throws {
        let (store, dir) = makeFileStore()
        let fixture = try XCTUnwrap(Bundle.module.url(
            forResource: "ride-v1", withExtension: "json", subdirectory: "Fixtures"))
        let archivedBytes = try Data(contentsOf: fixture)
        let ridesDir = dir.appendingPathComponent("rides", isDirectory: true)
        try FileManager.default.createDirectory(at: ridesDir, withIntermediateDirectories: true)
        let oldFile = ridesDir.appendingPathComponent("ride-v1-fixture.json")
        try archivedBytes.write(to: oldFile)
        let ride = makeRide(id: "ride-v1-fixture", name: "Current Ride")

        XCTAssertNil(store.ridePoints(ride.id))
        XCTAssertTrue(store.rideSummaries().isEmpty)
        XCTAssertEqual(try Data(contentsOf: oldFile), archivedBytes)

        try store.saveRide(ride)
        let relaunched = FileLibraryStore(directory: dir)
        XCTAssertEqual(relaunched.rideSummaries(), [ride.summary])
        XCTAssertEqual(relaunched.ridePoints(ride.id), ride.points)
        XCTAssertEqual(try Data(contentsOf: oldFile), archivedBytes)

        var renamed = ride.summary
        renamed.name = "Renamed Current Ride"
        relaunched.saveRideSummary(renamed)
        XCTAssertEqual(store.rideSummaries(), [renamed])
        XCTAssertEqual(store.ridePoints(ride.id), ride.points)
        XCTAssertEqual(try Data(contentsOf: oldFile), archivedBytes)

        store.deleteRide(ride.id)
        XCTAssertTrue(relaunched.rideSummaries().isEmpty)
        XCTAssertNil(relaunched.ridePoints(ride.id))
        XCTAssertEqual(try Data(contentsOf: oldFile), archivedBytes)
    }

    func testRideSaveReportsFilesystemFailureAndCanRetry() throws {
        for file in ["points.json", "summary.json"] {
            let (store, dir) = makeFileStore()
            let ride = makeRide()
            let blocker = dir.appendingPathComponent("rides/ride-1/\(file)")
            try FileManager.default.createDirectory(at: blocker, withIntermediateDirectories: true)

            XCTAssertThrowsError(try store.saveRide(ride))
            XCTAssertTrue(store.rideSummaries().isEmpty)
            XCTAssertTrue(store.syncedRideIDs().isEmpty)
            XCTAssertNil(store.ridePoints(ride.id), "a failed first publication exposes no generation")

            try FileManager.default.removeItem(at: blocker)
            try store.saveRide(ride)
            let relaunched = FileLibraryStore(directory: dir)
            XCTAssertEqual(relaunched.rideSummaries(), [ride.summary])
            XCTAssertEqual(relaunched.ridePoints(ride.id), ride.points)
        }
    }

    func testRideEncodingFailureLeavesBothFilesUnchanged() throws {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        try store.saveRide(ride)
        let points = try pointsURL(in: dir, id: "ride-1")
        let summary = dir.appendingPathComponent("rides/ride-1/summary.json")
        let pointBytes = try Data(contentsOf: points)
        let summaryBytes = try Data(contentsOf: summary)
        var invalid = ride
        invalid.points = []
        invalid.summary.distanceMeters = .nan

        XCTAssertThrowsError(try store.saveRide(invalid))
        XCTAssertEqual(try Data(contentsOf: points), pointBytes)
        XCTAssertEqual(try Data(contentsOf: summary), summaryBytes)
    }

    func testSyncedIDsSurviveRideDeleteAndRelaunch() throws {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        try store.saveRide(ride)
        store.markRideSynced(ride.id)
        store.deleteRide(ride.id)

        let relaunched = FileLibraryStore(directory: dir)
        XCTAssertTrue(relaunched.rideSummaries().isEmpty)
        // The marker outlives the ride: a deleted ride must not come back as new on the next sync.
        XCTAssertEqual(relaunched.syncedRideIDs(), [ride.id])
    }

    func testDeletedTombstonesSurviveRelaunch() throws {
        let (store, dir) = makeFileStore()
        let ride = makeRide()
        try store.saveRide(ride)
        store.markRideDeleted(ride.id)
        store.deleteRide(ride.id)

        // The tombstone keeps the device's copy out of the merged list after a relaunch.
        XCTAssertEqual(FileLibraryStore(directory: dir).deletedRideIDs(), [ride.id])
    }

    func testTrashedRideMarksSurviveRelaunchAndKeepTheFiles() throws {
        let (store, dir) = makeFileStore()
        let kept = makeRide()
        let recovered = makeRide(id: "ride-2", name: "Second")
        try store.saveRide(kept)
        try store.saveRide(recovered)
        let trashedAt = Date(timeIntervalSince1970: 1_700_000_000)
        store.markRideTrashed(kept.id, at: trashedAt)
        store.markRideTrashed(recovered.id, at: trashedAt.addingTimeInterval(60))
        store.unmarkRideTrashed(recovered.id)

        let relaunched = FileLibraryStore(directory: dir)
        XCTAssertEqual(relaunched.trashedRideIDs(), [kept.id: trashedAt])
        // Trash is a mark, not a move: the files stay readable, which makes Recover instant.
        XCTAssertEqual(Set(relaunched.rideSummaries().map(\.id)), [kept.id, recovered.id])
        XCTAssertEqual(relaunched.ridePoints(kept.id), kept.points)
    }

    func testAwkwardIDsStayDistinctOnDisk() throws {
        // Device ride ids are firmware-owned strings: path separators must not merge records.
        let (store, dir) = makeFileStore()
        let a = makeRide(id: "rides/2026-07-01 08:12")
        let b = makeRide(id: "rides_2026-07-01 08:12", name: "Twin")
        try store.saveRide(a)
        try store.saveRide(b)

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
