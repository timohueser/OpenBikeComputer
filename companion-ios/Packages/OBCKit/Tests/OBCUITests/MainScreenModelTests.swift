import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The main-screen model driven through `MockTransport`: the library-first Planned list, the
/// Tracked list, the live device cluster, search and delete. The sync state machine itself is
/// `RideSyncCoordinatorTests`.
@MainActor
final class MainScreenModelTests: XCTestCase {
    /// Sticky holds, terminal within a test, so a wait on `.done` or the confirm line cannot
    /// race its own expiry timer on a stalled runner.
    private static let stickyTiming = RideSyncCoordinator.Timing(
        syncDoneHold: .seconds(300),
        syncedLineHold: .seconds(300)
    )

    private func makeModel(
        _ scenario: Scenario,
        library: any LibraryStore = InMemoryLibraryStore(),
        seedLibrary: Bool = true,
        timing: RideSyncCoordinator.Timing = stickyTiming,
        now: @escaping () -> Date = Date.init
    ) -> (MainScreenModel, MockControl) {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        // Fast transfers: the pacing under test is the model's, not the mock's.
        control.throughputBytesPerSec = 200_000_000
        // What the composition root does for every mock run: the Planned list is library-first.
        if seedLibrary { control.seedLibrary(into: library) }
        let model = MainScreenModel(
            transport: MockTransport(control: control), library: library, syncTiming: timing,
            now: now)
        return (model, control)
    }

    /// A library record the way the import edge builds one.
    private func importedRecord(
        id: String = "imported-test",
        name: String = "Schwarzwald Tour · Tag 2"
    ) -> PlannedRouteRecord {
        let points = [
            RoutePoint(coordinate: Coordinate(latitude: 48.0, longitude: 8.0), elevationMeters: 500),
            RoutePoint(coordinate: Coordinate(latitude: 48.3, longitude: 8.2), elevationMeters: 600),
            RoutePoint(coordinate: Coordinate(latitude: 48.5, longitude: 8.1), elevationMeters: 550),
        ]
        return PlannedRouteRecord(
            summary: RouteSummary(
                id: RouteID(id), name: name,
                distanceMeters: 88_000, elevationGainMeters: 1_400
            ),
            route: ImportedRoute(
                name: name, points: points,
                waypoints: [Waypoint(index: 0, name: "Start", distanceAlongMeters: 0,
                                     coordinate: Coordinate(latitude: 48, longitude: 8))]
            ),
            sourceFileName: "tag2.gpx",
            sourceFileData: Data("<gpx/>".utf8)
        )
    }

    /// A device link scoped to the default fixture device's identity, so a record seeded with it
    /// behaves like an upload committed against the mock device.
    private func mockLink(_ objectID: UInt16) -> DeviceRouteLink {
        DeviceRouteLink(
            serial: "OBC-24-000317", storeID: FixtureSet.defaultStoreID,
            objectID: DeviceObjectID(objectID))
    }

    private func startLoaded(_ model: MainScreenModel) async throws {
        model.start()
        try await waitFor("library load") { model.loadState == .loaded }
    }

    /// Load, then pull the device's rides in: the Tracked list is library-first, so a ride only
    /// becomes a row once it is synced. Waits for a real post-sync marker, never the pre-sync
    /// `.idle`, which would race the async sync task, then for the progress caption to come down.
    private func startSynced(_ model: MainScreenModel) async throws {
        try await startLoaded(model)
        model.sync.sync()
        try await waitFor("sync completes") {
            model.sync.syncState == .done || model.sync.upToDateToastVisible
        }
        // Under sticky timing `.done` never yields to `.idle` in test time, so "settled" is the
        // progress caption coming down.
        try await waitFor("sync settles") { model.sync.syncProgress == nil }
    }

    // MARK: Stream lifecycle

    /// The state and battery streams never finish, so the loops must hold the model weakly and it
    /// can deallocate with the streams still live.
    func testStreamTasksDoNotRetainTheModel() async throws {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        weak var leaked: MainScreenModel?
        do {
            let model = MainScreenModel(transport: MockTransport(control: control))
            // Let the one-shot tasks finish: bounded work may hold the model strongly, the stream
            // loops never.
            try await startLoaded(model)
            try await waitFor("device identity") { model.deviceName == "Trailhead" }
            leaked = model
        }
        // The model's last strong ref is gone; push an event through the still-open streams, so
        // a strongly-capturing loop would show up as a live ref.
        control.connection = .outOfRange
        for _ in 0..<10 { await Task.yield() }
        XCTAssertNil(leaked, "the stream loops must hold the model weakly")
    }

    // MARK: Lists + device cluster

    func testLoadPopulatesListsAndIdentityFromFixtures() async throws {
        let (model, _) = makeModel(.happyPath)
        try await startLoaded(model)

        XCTAssertEqual(model.routes.count, 5)
        XCTAssertEqual(model.routes.first?.name, "Kettle Moraine Loop")
        // A fixture the device holds proves up only once the scope settles, because the proof
        // needs the connected identity and a matching catalog CRC. Poll, do not assert at once.
        try await waitFor("kettle badge proves") { model.isUploaded(RouteID("kettle-moraine-loop")) }
        XCTAssertFalse(model.isUploaded(RouteID("blue-mounds-backroads")))
        // Tracked is library-first: the device's rides are not rows until they are synced.
        XCTAssertTrue(model.rides.isEmpty)
        model.sync.sync()
        try await waitFor("rides synced in") { model.rides.count == 4 }
        XCTAssertEqual(model.rides[1].name, "Sunday Coffee Spin")
        try await waitFor("device identity") { model.deviceName == "Trailhead" }
        try await waitFor("battery replay") { model.battery == 82 }
        XCTAssertEqual(model.connection, .connected)
        XCTAssertFalse(model.showsDisconnectedBanner)
    }

    func testBatteryNudgeFlowsLive() async throws {
        let (model, control) = makeModel(.happyPath)
        try await startLoaded(model)
        try await waitFor("battery replay") { model.battery == 82 }

        control.battery = 55
        try await waitFor("battery nudge") { model.battery == 55 }
    }

    func testConnectionChangeDrivesTheBanner() async throws {
        let (model, control) = makeModel(.happyPath)
        try await startLoaded(model)

        control.connection = .outOfRange
        try await waitFor("S4 banner") { model.showsDisconnectedBanner }
        control.connection = .connected
        try await waitFor("silent reconnect") { !model.showsDisconnectedBanner }
    }

    func testReadErrorThenRetrySucceeds() async throws {
        let (model, _) = makeModel(.readError)
        model.start()
        try await waitFor("S3 failure") { model.loadState == .failed }

        model.reload()   // the one-shot fault is spent — retry succeeds
        try await waitFor("retry load") { model.loadState == .loaded }
        XCTAssertEqual(model.routes.count, 5)
    }

    // MARK: Search

    func testSearchFiltersBothTabsCaseInsensitively() async throws {
        let (model, _) = makeModel(.happyPath)
        try await startSynced(model)   // Tracked is library-first: sync so rides exist to filter

        model.searchText = "sugar"
        XCTAssertEqual(model.filteredRoutes.map(\.name), ["Sugar River Trail"])

        model.searchText = "COFFEE"
        XCTAssertEqual(model.filteredRides.map(\.name), ["Sunday Coffee Spin"])
        XCTAssertTrue(model.filteredRoutes.isEmpty)   // H6 on the other tab

        model.searchText = ""
        XCTAssertEqual(model.filteredRoutes.count, 5)
        XCTAssertEqual(model.filteredRides.count, 4)
    }

    // MARK: Sync and the lists

    func testRideAddedOnDeviceSurfacesInTheListAfterSync() async throws {
        let (model, control) = makeModel(.happyPath)
        try await startSynced(model)

        control.emit(.rideAdded(RideSummary(
            id: RideID("ride-new"),
            name: "Lunch Loop",
            date: Date(),
            distanceMeters: 18_000,
            movingTime: 2_800,
            averageSpeedMps: 6.4
        )))

        model.sync.sync()
        try await waitFor("one new ride") { model.sync.lastSyncCount == 1 }
        XCTAssertEqual(model.rides.first?.name, "Lunch Loop")
    }

    func testRideGeometryIsAvailableAfterSync() async throws {
        let (model, _) = makeModel(.happyPath)
        try await startLoaded(model)

        model.sync.sync()
        try await waitFor("first sync") { model.sync.lastSyncCount != nil }

        let ride = model.rides.first { $0.name == "Kettle Moraine Loop" }
        let geometry = ride.flatMap { model.rideGeometry(for: $0.id) }
        XCTAssertNotNil(geometry, "a synced ride's points should be available for the map")
        XCTAssertFalse(geometry?.isEmpty ?? true)

        XCTAssertNil(model.rideGeometry(for: RideID("nonexistent")))
    }

    /// One banner at a time: while a dropped sync waits for Resume the link banner yields to the
    /// interruption's, although the link really is out of range.
    func testInterruptionBannerOutranksTheDisconnectedBanner() async throws {
        let (model, control) = makeModel(.happyPath)
        try await startLoaded(model)

        control.dropTransfer(atFraction: 0.5)
        model.sync.sync()
        try await waitFor("H10 raised") { model.sync.syncInterruption != nil }

        XCTAssertEqual(model.connection, .outOfRange)
        XCTAssertFalse(model.showsDisconnectedBanner,
                       "the H10 banner tells the link story — never two banners")
    }

    /// The default mock `listRides` returns fixture summaries that already carry a preview,
    /// richer than the wire, so this test injects a ride shaped like a real device's: a
    /// preview-less list summary over a payload that does have geometry.
    func testSyncedOnDeviceRideShowsATrackPreview() async throws {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(.happyPath, library: library, seedLibrary: false)
        // Reshape the ride catalog to match the wire: no preview in the summary, real points
        // in the payload.
        var fixtures = control.fixtures
        let ridePoints = [
            RidePoint(timestamp: Date(timeIntervalSince1970: 0),
                      coordinate: Coordinate(latitude: 47.0, longitude: 8.0), elevationMeters: 500),
            RidePoint(timestamp: Date(timeIntervalSince1970: 60),
                      coordinate: Coordinate(latitude: 47.01, longitude: 8.02), elevationMeters: 540),
            RidePoint(timestamp: Date(timeIntervalSince1970: 120),
                      coordinate: Coordinate(latitude: 47.02, longitude: 8.05), elevationMeters: 520),
        ]
        fixtures.rides = [RideEntry(
            summary: RideSummary(
                id: RideID("42"), name: "Wire Ride", date: Date(timeIntervalSince1970: 0),
                distanceMeters: 12_000, movingTime: 2_400, averageSpeedMps: 5, climbMeters: 60,
                trackPreview: nil  // the §7.4 list carries no geometry
            ),
            points: ridePoints)]
        control.fixtures = fixtures

        try await startLoaded(model)

        XCTAssertNil(model.rides.first { $0.id == RideID("42") },
                     "an un-synced device ride isn't listed")

        model.sync.sync()
        try await waitFor("sync done") { model.sync.syncState == .done }

        let after = model.rides.first { $0.id == RideID("42") }
        XCTAssertNotNil(after, "the synced ride is now a row")
        XCTAssertFalse(after?.trackPreview?.points.isEmpty ?? true,
                       "a synced ride shows the downloaded track, not the placeholder")
    }

    // MARK: Protocol version

    func testProtocolMismatchSurfacesAndDisablesSync() async throws {
        let (model, control) = makeModel(.happyPath)
        control.deviceInfo = DeviceInfo(
            name: "Trailhead", firmwareVersion: "9.9.9",
            protocolVersion: OBCProtocol.version + 1
        )
        try await startLoaded(model)
        try await waitFor("mismatch surfaces") { model.protocolMismatch != nil }
        XCTAssertEqual(
            model.protocolMismatch,
            .init(expected: OBCProtocol.version, found: OBCProtocol.version + 1)
        )

        // Sync must not start a transfer: the coordinator asks the model through `canSync`.
        model.sync.sync()
        let stayedBlocked = await neverHolds({
            model.sync.syncProgress != nil || model.sync.upToDateToastVisible
                || model.sync.syncState == .done
        }, for: .milliseconds(80))
        XCTAssertTrue(stayedBlocked, "a protocol mismatch must prevent progress and success")
        XCTAssertEqual(model.sync.syncState, .idle)
        XCTAssertNil(model.sync.syncProgress)
        XCTAssertFalse(model.sync.upToDateToastVisible)
        // A reload after the mismatch is known must not decode the device either.
        model.reload()
        XCTAssertEqual(model.loadState, .loaded)
    }

    func testMatchingProtocolVersionDoesNotFlag() async throws {
        let (model, _) = makeModel(.happyPath)   // fixtures report OBCProtocol.version
        try await startLoaded(model)
        try await waitFor("device identity") { model.deviceName == "Trailhead" }
        XCTAssertNil(model.protocolMismatch)
        model.sync.sync()
        try await waitFor("sync runs") {
            model.sync.syncState == .syncing || model.sync.lastSyncCount != nil
        }
    }

    // MARK: Delete

    func testDeleteRouteRemovesFromLibraryButNeverFromDevice() async throws {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(.happyPath, library: library)
        try await startLoaded(model)

        let id = model.routes[0].id
        model.deleteRoute(id)
        XCTAssertEqual(model.routes.count, 4)   // optimistic removal
        XCTAssertFalse(library.plannedRoutes().contains { $0.id == id }, "delete reaches the library")
        // A copy already on the device stays there.
        XCTAssertTrue(control.fixtures.routes.contains { $0.deviceObjectID != nil })

        model.reload()
        try await waitFor("reload") { model.loadState == .loaded }
        XCTAssertFalse(model.routes.contains { $0.id == id }, "the device copy must not re-list it")
    }

    func testDeleteRideMovesToTrashAndStaysOutOfNewCounts() async throws {
        let library = InMemoryLibraryStore()
        let (model, _) = makeModel(.happyPath, library: library)
        try await startSynced(model)
        XCTAssertEqual(model.rides.count, 4)

        let id = model.rides[0].id
        model.deleteRide(id)
        XCTAssertEqual(model.rides.count, 3)
        // Delete is a move to Recently Deleted, not a destroy: the stored files stay for Recover.
        XCTAssertEqual(model.trashedRides.map(\.id), [id])
        XCTAssertNotNil(library.ridePoints(id), "the trashed ride's tracklog must survive")

        // The trashed ride must not come back as a "new" sync count: its id stays marked synced,
        // and it never re-lists, because the device's own copy is untouched by design.
        model.sync.sync()
        try await waitFor("re-sync settles") {
            model.sync.upToDateToastVisible || model.sync.syncState == .done
        }
        XCTAssertFalse(model.rides.contains { $0.id == id }, "trashed ride resurrected by sync")
        XCTAssertEqual(model.rides.count, 3)

        model.reload()
        try await waitFor("reload") { model.loadState == .loaded }
        XCTAssertFalse(model.rides.contains { $0.id == id }, "trashed ride resurrected by reload")
        XCTAssertEqual(model.rides.count, 3)
    }

    func testTrashedRideStaysTrashedAcrossRelaunch() async throws {
        let library = InMemoryLibraryStore()
        let (first, _) = makeModel(.happyPath, library: library)
        try await startSynced(first)
        let id = first.rides[0].id
        first.deleteRide(id)

        let (relaunched, _) = makeModel(.happyPath, library: library)
        try await startLoaded(relaunched)
        XCTAssertFalse(relaunched.rides.contains { $0.id == id })
        XCTAssertEqual(relaunched.trashedRides.map(\.id), [id], "trash lost across relaunch")

        relaunched.sync.sync()
        try await waitFor("sync settles") {
            relaunched.sync.syncState == .done || relaunched.sync.upToDateToastVisible
        }
        XCTAssertFalse(relaunched.rides.contains { $0.id == id }, "trash mark lost across relaunch")
    }

    func testRecoverRideRestoresTheRow() async throws {
        let library = InMemoryLibraryStore()
        let (model, _) = makeModel(.happyPath, library: library)
        try await startSynced(model)
        let id = model.rides[1].id
        let countBefore = model.rides.count

        model.deleteRide(id)
        model.recoverRide(id)
        XCTAssertEqual(model.rides.count, countBefore)
        XCTAssertEqual(model.rides[1].id, id, "a recovered ride returns to its date slot")
        XCTAssertTrue(model.trashedRides.isEmpty)
        XCTAssertNotNil(model.rideGeometry(for: id), "the tracklog survived the round trip")

        let (relaunched, _) = makeModel(.happyPath, library: library)
        try await startLoaded(relaunched)
        XCTAssertTrue(relaunched.rides.contains { $0.id == id }, "recovery lost across relaunch")
        XCTAssertTrue(relaunched.trashedRides.isEmpty)
    }

    /// Files gone, tombstone durable: a later sync must neither re-download nor re-list the ride.
    func testDeleteRideForeverRemovesFilesAndTombstones() async throws {
        let library = InMemoryLibraryStore()
        let (model, _) = makeModel(.happyPath, library: library)
        try await startSynced(model)
        let id = model.rides[0].id

        model.deleteRide(id)
        model.deleteRideForever(id)
        XCTAssertTrue(model.trashedRides.isEmpty)
        XCTAssertNil(library.ridePoints(id), "the tracklog dies with the permanent delete")

        let (relaunched, _) = makeModel(.happyPath, library: library)
        try await startLoaded(relaunched)
        XCTAssertFalse(relaunched.rides.contains { $0.id == id })
        XCTAssertTrue(relaunched.trashedRides.isEmpty)
        relaunched.sync.sync()
        try await waitFor("sync settles") {
            relaunched.sync.syncState == .done || relaunched.sync.upToDateToastVisible
        }
        XCTAssertFalse(relaunched.rides.contains { $0.id == id }, "purged ride resurrected by sync")
    }

    func testExpiredTrashIsPurgedAtStart() async throws {
        let library = InMemoryLibraryStore()
        let (first, _) = makeModel(.happyPath, library: library)
        try await startSynced(first)
        let expired = first.rides[0].id
        let fresh = first.rides[1].id
        first.deleteRide(expired)

        // A relaunch a day short of the window: still in the trash.
        let almost = Date().addingTimeInterval(
            TimeInterval(MainScreenModel.trashRetentionDays - 1) * 86_400)
        let (kept, _) = makeModel(.happyPath, library: library, now: { almost })
        try await startLoaded(kept)
        XCTAssertEqual(kept.trashedRides.map(\.id), [expired])
        kept.deleteRide(fresh)

        // Past the window: the first is purged, the second survives.
        let later = Date().addingTimeInterval(
            TimeInterval(MainScreenModel.trashRetentionDays + 1) * 86_400)
        let (relaunched, _) = makeModel(.happyPath, library: library, now: { later })
        try await startLoaded(relaunched)
        XCTAssertEqual(relaunched.trashedRides.map(\.id), [fresh])
        XCTAssertNil(library.ridePoints(expired), "expired trash must be removed for good")
        XCTAssertNotNil(library.ridePoints(fresh))
    }

    // MARK: Rename and import landing

    func testRenameUpdatesTheLists() async throws {
        let (model, _) = makeModel(.happyPath)
        try await startSynced(model)

        model.renameRoute(model.routes[0].id, to: "Kettle Gravel Day")
        XCTAssertEqual(model.routes[0].name, "Kettle Gravel Day")

        model.renameRide(model.rides[1].id, to: "Sunday Espresso Spin")
        XCTAssertEqual(model.rides[1].name, "Sunday Espresso Spin")
    }

    func testAddImportedRouteLandsOnTopOfPlannedAndKeepsItsDetail() async throws {
        let (model, _) = makeModel(.happyPath)
        try await startLoaded(model)
        model.tab = .tracked

        let record = importedRecord()
        model.addImportedRoute(record)

        XCTAssertEqual(model.routes.count, 6)
        XCTAssertEqual(model.routes[0].id, record.id)
        XCTAssertEqual(model.tab, .planned, "saving lands the user on the Planned list")

        // Reopening must not lose the parsed data, and a rename must show in it.
        model.renameRoute(record.id, to: "Schwarzwald Day 2")
        let kept = model.importedDetail(for: record.id)
        XCTAssertEqual(kept?.waypoints.count, 1)
        XCTAssertFalse(kept?.elevationProfile.isEmpty ?? true, "profile derives from the saved geometry")
        XCTAssertEqual(kept?.summary.name, "Schwarzwald Day 2")

        model.deleteRoute(record.id)
        XCTAssertNil(model.importedDetail(for: record.id))
    }

    // MARK: "On device" badge

    func testUploadCompletionLightsTheOnDeviceBadge() async throws {
        let (model, _) = makeModel(.happyPath)
        try await startLoaded(model)
        try await waitFor("the identity scope") { model.connectedScope != nil }
        let record = importedRecord()
        model.addImportedRoute(record)
        XCTAssertFalse(model.isUploaded(record.id), "a fresh import isn't on the device")

        // What the upload sheet does with the device-assigned id and the payload fingerprint.
        model.markRouteUploaded(record.id, objectID: DeviceObjectID(7), crc32: RouteObjectCodec.payloadCRC(for: record))
        XCTAssertTrue(model.isUploaded(record.id))
        XCTAssertEqual(model.onDeviceState(record.id), .upToDate)
        XCTAssertEqual(model.plannedDeviceObjectID(for: record.id), DeviceObjectID(7))

        model.deleteRoute(record.id)
        XCTAssertFalse(model.isUploaded(record.id), "deleting clears the badge")
    }

    /// The mock's `deviceDeletesRoute` sends exactly the wire sequence a real on-device delete
    /// sends: the catalog forgets the copy, then the store-change edge. The badge clears live,
    /// with no reconnect and no manual refresh.
    func testOnDeviceDeleteClearsTheBadgeLive() async throws {
        let (model, control) = makeModel(.happyPath)
        try await startLoaded(model)
        try await waitFor("the fixture proves on-device") { model.isUploaded(RouteID("kettle-moraine-loop")) }

        control.deviceDeletesRoute(DeviceObjectID(7)) // kettle-moraine-loop's device copy
        try await waitFor("badge clears on storeChanged") { !model.isUploaded(RouteID("kettle-moraine-loop")) }
        // The record survives; only its device link is gone, so a re-upload is offered.
        XCTAssertTrue(model.routes.contains { $0.id == RouteID("kettle-moraine-loop") })
    }

    /// A store change during an in-flight catalog read queues one follow-up pass. It must not
    /// cancel the opened read: on BLE that leaves its closing result to be mistaken for the
    /// next upload.
    func testStoreChangeBurstDoesNotCancelAnInFlightCatalogRead() async throws {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        let library = InMemoryLibraryStore()
        control.seedLibrary(into: library)
        let transport = ObservedMockTransport(control: control, gateFirstRouteCatalog: true)
        let model = MainScreenModel(transport: transport, library: library)

        model.start()
        defer { transport.releaseFirstRouteCatalog() }
        try await waitFor("opened catalog read and change subscription") {
            transport.routeCatalogStartedCount == 1 && transport.catalogChangesObserved
        }
        control.deviceDeletesRoute(DeviceObjectID(7))
        control.deviceDeletesRoute(DeviceObjectID(8))
        transport.releaseFirstRouteCatalog()

        try await waitFor("follow-up reconcile") {
            transport.routeCatalogCompletedCount >= 2
                && model.loadState == .loaded
                && !model.isUploaded(RouteID("kettle-moraine-loop"))
        }
        XCTAssertEqual(control.cancelledRouteCatalogReadCount, 0)
    }

    /// An uploaded route is up to date until its content moves: a rename out-dates it, because
    /// the name rides in the payload, and the next committed upload brings it current again
    /// under the same object id.
    func testRenameOutdatesTheDeviceCopyAndReuploadHeals() async throws {
        let (model, _) = makeModel(.happyPath)
        try await startLoaded(model)
        try await waitFor("the identity scope") { model.connectedScope != nil }
        let record = importedRecord()
        model.addImportedRoute(record)
        model.markRouteUploaded(record.id, objectID: DeviceObjectID(7), crc32: RouteObjectCodec.payloadCRC(for: record))
        XCTAssertEqual(model.onDeviceState(record.id), .upToDate)

        model.renameRoute(record.id, to: "Schwarzwald Tour (final)")
        XCTAssertEqual(model.onDeviceState(record.id), .outdated, "the name rides in the payload")
        XCTAssertEqual(model.plannedDeviceObjectID(for: record.id), DeviceObjectID(7), "the device link survives the rename")

        // The next upload commits the renamed payload, so the route is current again.
        var renamed = record
        renamed.summary.name = "Schwarzwald Tour (final)"
        model.markRouteUploaded(record.id, objectID: DeviceObjectID(7), crc32: RouteObjectCodec.payloadCRC(for: renamed))
        XCTAssertEqual(model.onDeviceState(record.id), .upToDate)
    }

    /// A link with no committed fingerprint is unproven: the app cannot verify what the linked
    /// id points at, so it shows no badge. The route still offers Upload, so the next push
    /// self-heals it with a real fingerprint.
    func testUnknownFingerprintShowsNoBadge() async throws {
        let library = InMemoryLibraryStore()
        var record = importedRecord()
        record.deviceLink = mockLink(7)   // linked, but `uploadedCRC32` stays nil
        library.savePlannedRoute(record)

        let (model, _) = makeModel(.happyPath, library: library)
        try await startLoaded(model)
        try await waitFor("the identity scope") { model.connectedScope != nil }
        XCTAssertEqual(model.onDeviceState(record.id), .notOnDevice)
        XCTAssertFalse(model.isUploaded(record.id), "no fingerprint proves nothing — no badge")
    }

    func testSeededDeviceCopiesBootUpToDate() async throws {
        let (model, _) = makeModel(.happyPath)
        try await startLoaded(model)
        try await waitFor("the proven up-to-date badge") {
            model.onDeviceState(RouteID("kettle-moraine-loop")) == .upToDate
        }
    }

    /// A fresh model re-proves the badge against the catalog CRC, never on link presence alone,
    /// and threads the object id for replace-by-id, once the identity read settles on the
    /// matching identity.
    func testSeededDeviceCopyKeepsItsProvenBadgeAndReplaceTarget() async throws {
        let (model, _) = makeModel(.happyPath)
        try await startLoaded(model)
        try await waitFor("the badge proves") { model.isUploaded(RouteID("kettle-moraine-loop")) }
        XCTAssertEqual(model.onDeviceState(RouteID("kettle-moraine-loop")), .upToDate)
        try await waitFor("the identity scope") { model.connectedScope != nil }
        XCTAssertEqual(model.plannedDeviceObjectID(for: RouteID("kettle-moraine-loop")), DeviceObjectID(7))
    }

    /// A copy deleted out from under us clears the stored link, and the badge, on the next reload.
    func testReloadClearsTheBadgeWhenTheDeviceNoLongerHoldsTheRoute() async throws {
        let library = InMemoryLibraryStore()
        var record = importedRecord()
        record.deviceLink = mockLink(999)   // no fixture device object has this id
        library.savePlannedRoute(record)

        let (model, _) = makeModel(.happyPath, library: library)
        try await startLoaded(model)
        // Wait on the reconcile write itself, not on the badge: `isUploaded` is proof-based, so
        // it goes dark as soon as the catalog fails to prove object 999, which happens before
        // the scope settles and so before anything is cleared.
        try await waitFor("the stale link to clear from the store") {
            library.plannedRoutes().first { $0.id == record.id }?.deviceLink == nil
        }
        XCTAssertFalse(model.isUploaded(record.id), "the badge goes dark with the link")
        XCTAssertNil(model.plannedDeviceObjectID(for: record.id))
        XCTAssertNil(library.plannedRoutes().first { $0.id == record.id }?.deviceLink,
                     "the cleared link persists")
    }

    func testUploadedRouteKeepsItsBadgeThroughReload() async throws {
        let (model, control) = makeModel(.happyPath)
        try await startLoaded(model)
        let record = importedRecord()
        model.addImportedRoute(record)

        let handle = MockTransport(control: control).uploadRoute(RouteBlob(
            summary: record.summary, payload: Data([1, 2, 3])
        ))
        guard await handle.outcome == .completed, let objectID = await handle.assignedObjectID else {
            return XCTFail("mock upload must commit and assign an id")
        }
        model.markRouteUploaded(record.id, objectID: objectID, crc32: CRC32.checksum(Data([1, 2, 3])))
        XCTAssertTrue(model.isUploaded(record.id))

        model.reload()
        try await waitFor("reload") { model.loadState == .loaded }
        XCTAssertTrue(model.isUploaded(record.id), "the device lists the fresh copy — the badge survives reconcile")
    }

    func testReconnectReloadsAndReconcilesTheBadge() async throws {
        let (model, control) = makeModel(.happyPath)
        try await startLoaded(model)
        try await waitFor("the fixture proves on-device") { model.isUploaded(RouteID("kettle-moraine-loop")) }

        // The device loses the copy; nothing tells the model, so the badge stays lit for now.
        try? await MockTransport(control: control).deleteRoute(DeviceObjectID(7))
        XCTAssertTrue(model.isUploaded(RouteID("kettle-moraine-loop")))

        control.connection = .outOfRange
        try await waitFor("S4 banner") { model.showsDisconnectedBanner }
        control.connection = .connected
        try await waitFor("reconnect reconcile") { !model.isUploaded(RouteID("kettle-moraine-loop")) }
    }

    // MARK: Identity-verified badges and adopt-by-content

    /// A link that survived scoping but points at an object the device has since replaced, so
    /// the catalog CRC disagrees with our committed fingerprint, is dropped and never shown up
    /// to date on presence.
    func testCRCMismatchDropsTheLinkNeverACheckmark() async throws {
        let library = InMemoryLibraryStore()
        var record = importedRecord(id: "lib-mismatch")
        record.deviceLink = mockLink(7)        // object 7 exists (the kettle copy)…
        record.uploadedCRC32 = 0xDEAD_BEEF     // …but the device doesn't hold this
        library.savePlannedRoute(record)

        let (model, _) = makeModel(.happyPath, library: library)
        try await startLoaded(model)
        try await waitFor("the mismatched link drops") {
            library.plannedRoutes().first { $0.id == record.id }?.deviceLink == nil
        }
        XCTAssertEqual(model.onDeviceState(record.id), .notOnDevice)
        XCTAssertNil(model.plannedDeviceObjectID(for: record.id),
                     "a mismatched link must not thread a replace target (the wrong-route-overwrite bug)")
    }

    /// The library kept the route but lost its device link, and the device still holds an
    /// identical copy, so adoption re-links it with no re-upload and a later push replaces that
    /// object by id instead of duplicating it.
    func testAdoptByContentHealsAnAppReinstall() async throws {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(.happyPath, library: library, seedLibrary: false)
        // The device holds this content under object 900; the phone kept the route but has no
        // link to it.
        let template = importedRecord(id: "lib-adopt", name: "Reinstalled Ridge")
        var fixtures = control.fixtures
        fixtures.routes.append(RouteEntry(
            summary: template.summary, points: template.route.points,
            waypoints: template.route.waypoints, payloadByteCount: 100,
            deviceObjectID: DeviceObjectID(900)))
        control.fixtures = fixtures
        library.savePlannedRoute(template)   // no deviceLink

        try await startLoaded(model)
        try await waitFor("adoption re-links the identical copy") {
            library.plannedRoutes().first { $0.id == template.id }?.deviceLink != nil
        }
        try await waitFor("the identity scope") { model.connectedScope != nil }
        XCTAssertEqual(model.plannedDeviceObjectID(for: template.id), DeviceObjectID(900),
                       "the adopted id threads replace-by-id")
        XCTAssertEqual(model.onDeviceState(template.id), .upToDate,
                       "adopted content is byte-identical → up to date, no upload needed")
    }

    func testAdoptedRouteUploadsAsReplaceNotDuplicate() async throws {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(.happyPath, library: library, seedLibrary: false)
        let template = importedRecord(id: "lib-adopt-upload", name: "Adopt Then Edit")
        var fixtures = control.fixtures
        fixtures.routes.append(RouteEntry(
            summary: template.summary, points: template.route.points,
            waypoints: template.route.waypoints, payloadByteCount: 100,
            deviceObjectID: DeviceObjectID(901)))
        control.fixtures = fixtures
        library.savePlannedRoute(template)

        try await startLoaded(model)
        try await waitFor("adoption") { model.plannedDeviceObjectID(for: template.id) == DeviceObjectID(901) }
        let deviceRouteCountBefore = control.fixtures.routes.filter { $0.deviceObjectID != nil }.count

        // The upload the app would send targets the adopted id, so it replaces in place.
        let blob = RouteBlob(
            summary: template.summary, waypoints: template.route.waypoints,
            payload: Data([9, 9, 9]), targetObjectID: model.plannedDeviceObjectID(for: template.id))
        let handle = MockTransport(control: control).uploadRoute(blob)
        _ = await handle.outcome
        XCTAssertEqual(
            control.fixtures.routes.filter { $0.deviceObjectID != nil }.count,
            deviceRouteCountBefore, "an adopted upload replaces by id — never a duplicate")
    }

    /// A route renamed while the phone holds no link still finds its device copy: the catalog
    /// reports the name that copy was stored under, so the fingerprint is reconstructible
    /// without a download. Without it the next send creates a duplicate.
    func testRenamedRouteAdoptsItsDeviceCopyAndReplacesIt() async throws {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(.happyPath, library: library, seedLibrary: false)
        let onDevice = importedRecord(id: "lib-renamed", name: "Kettle Moraine")
        var fixtures = control.fixtures
        fixtures.routes.append(RouteEntry(
            summary: onDevice.summary, points: onDevice.route.points,
            waypoints: onDevice.route.waypoints, payloadByteCount: 100,
            deviceObjectID: DeviceObjectID(903)))
        control.fixtures = fixtures
        // The library holds the same route under a new name, and no link.
        var renamed = onDevice
        renamed.summary.name = "Kettle Moraine (long way)"
        renamed.route = ImportedRoute(
            name: renamed.summary.name, points: onDevice.route.points, waypoints: onDevice.route.waypoints)
        library.savePlannedRoute(renamed)

        try await startLoaded(model)
        try await waitFor("the renamed route adopts its device copy") {
            model.plannedDeviceObjectID(for: renamed.id) == DeviceObjectID(903)
        }
        XCTAssertEqual(model.onDeviceState(renamed.id), .outdated,
                       "the device holds it under the old name — on the device, out of date")

        // The send that follows targets that object instead of creating a twin.
        let deviceRouteCountBefore = control.fixtures.routes.filter { $0.deviceObjectID != nil }.count
        let blob = RouteBlob(
            summary: renamed.summary, waypoints: renamed.route.waypoints,
            payload: RouteObjectCodec.encode(
                points: renamed.route.points, waypoints: renamed.route.waypoints,
                name: renamed.summary.name, bikeType: renamed.bikeType),
            targetObjectID: model.plannedDeviceObjectID(for: renamed.id))
        _ = await MockTransport(control: control).uploadRoute(blob).outcome
        XCTAssertEqual(
            control.fixtures.routes.filter { $0.deviceObjectID != nil }.count,
            deviceRouteCountBefore, "a renamed send replaces by id — never a duplicate")
        XCTAssertEqual(
            control.fixtures.routes.first { $0.summary.id == renamed.id }?.deviceObjectID,
            DeviceObjectID(903), "…and the device copy kept its id, so the send was a replace")
    }

    /// A device whose route sidecar has not filled yet reports `crc32 = 0`. Zero proves nothing,
    /// so there is no badge; it is not a disproof either, so the link is kept.
    func testUnknownCatalogCRCProvesNothingButKeepsTheLink() async throws {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(.happyPath, library: library, seedLibrary: false)
        var fixtures = control.fixtures
        fixtures.routes.append(RouteEntry(
            summary: RouteSummary(id: RouteID("dev-unknown"), name: "Sidecar Pending",
                                  distanceMeters: 1_000, elevationGainMeters: 10),
            points: importedRecord().route.points, payloadByteCount: 100,
            deviceObjectID: DeviceObjectID(902), crc32: 0))   // explicit unknown
        control.fixtures = fixtures
        var record = importedRecord(id: "lib-unknown")
        record.deviceLink = mockLink(902)
        record.uploadedCRC32 = 0x1234_5678
        library.savePlannedRoute(record)

        try await startLoaded(model)
        try await waitFor("the identity scope") { model.connectedScope != nil }
        XCTAssertEqual(model.onDeviceState(record.id), .notOnDevice, "crc32 = 0 proves nothing — no badge")
        let keptUnknownLink = await neverHolds({
            library.plannedRoutes().first { $0.id == record.id }?.deviceLink == nil
        }, for: .milliseconds(50))
        XCTAssertTrue(keptUnknownLink, "an unknown CRC must not clear the link")
        XCTAssertNotNil(library.plannedRoutes().first { $0.id == record.id }?.deviceLink,
                        "an unknown CRC is not a disproof — the link is kept")
    }

    /// A link minted on another device is invisible and untouchable while connected to this one:
    /// no badge, and this device's catalog cannot clear it.
    func testDeviceBCatalogNeverTouchesDeviceALinks() async throws {
        let library = InMemoryLibraryStore()
        let foreignLink = DeviceRouteLink(
            serial: "OBC-OTHER-DEVICE", storeID: FixtureSet.defaultStoreID, objectID: DeviceObjectID(7))
        var record = importedRecord(id: "lib-deviceB")
        record.deviceLink = foreignLink
        record.uploadedCRC32 = 0xAAAA_BBBB
        library.savePlannedRoute(record)

        let (model, _) = makeModel(.happyPath, library: library)   // connects to device A
        try await startLoaded(model)
        try await waitFor("the identity scope") { model.connectedScope != nil }
        XCTAssertEqual(model.onDeviceState(record.id), .notOnDevice, "device B's link never badges on device A")
        XCTAssertNil(model.plannedDeviceObjectID(for: record.id), "…and never threads a replace target")
        let keptForeignLink = await neverHolds({
            library.plannedRoutes().first { $0.id == record.id }?.deviceLink != foreignLink
        }, for: .milliseconds(50))
        XCTAssertTrue(keptForeignLink, "device A must not rewrite device B's link")
        XCTAssertEqual(
            library.plannedRoutes().first { $0.id == record.id }?.deviceLink, foreignLink,
            "device A's reconcile must not clear device B's link")
    }

    func testPlannedRouteNamedFindsACollisionCaseInsensitively() async throws {
        let (model, _) = makeModel(.happyPath)
        try await startLoaded(model)
        model.addImportedRoute(importedRecord(name: "Schwarzwald Tour · Tag 2"))
        XCTAssertNotNil(model.plannedRoute(named: "schwarzwald tour · tag 2"))
        XCTAssertNil(model.plannedRoute(named: "A Different Route"))
    }

    // MARK: The library store, across "relaunches"

    /// An import saved before, or without, a device survives a relaunch, rename included.
    func testImportedRouteSurvivesRelaunch() async throws {
        let library = InMemoryLibraryStore()
        let (first, _) = makeModel(.happyPath, library: library)
        try await startLoaded(first)
        let record = importedRecord()
        first.addImportedRoute(record)
        first.renameRoute(record.id, to: "Schwarzwald Day 2")

        let (relaunched, _) = makeModel(.happyPath, library: library)
        try await startLoaded(relaunched)

        XCTAssertEqual(relaunched.routes.count, 6, "the saved import joins the seeded five")
        XCTAssertEqual(relaunched.routes.first?.id, record.id, "the newest save stays on top")
        XCTAssertEqual(relaunched.routes.first?.name, "Schwarzwald Day 2")
        let kept = relaunched.importedDetail(for: record.id)
        XCTAssertEqual(kept?.waypoints.count, 1, "the parsed detail is derivable after relaunch")

        relaunched.deleteRoute(record.id)
        XCTAssertFalse(library.plannedRoutes().contains { $0.id == record.id }, "delete reaches the library")
    }

    /// The store is why a failed device read degrades to browsable content instead of an empty
    /// error screen.
    func testStoreSeededListsStayBrowsableWhenTheReadFails() async throws {
        let library = InMemoryLibraryStore()
        let record = importedRecord()
        library.savePlannedRoute(record)
        let ride = Ride(
            summary: RideSummary(id: RideID("ride-kept"), name: "Kept Ride",
                                 date: Date(), distanceMeters: 20_000),
            points: []
        )
        library.saveRide(ride)
        library.markRideSynced(ride.id)

        let (model, _) = makeModel(.readError, library: library, seedLibrary: false)
        model.start()
        try await waitFor("read failure") { model.loadState == .failed }

        XCTAssertEqual(model.routes.map(\.id), [record.id], "planned stays browsable")
        XCTAssertEqual(model.rides.map(\.id), [ride.id], "tracked stays browsable")
    }

    // MARK: Desired-name reconcile

    /// The launch connection reconciles after the first load, and every regained link reconciles
    /// again, so a rename whose config write never landed converges without any user action.
    func testConnectRunsTheDesiredNameReconcile() async throws {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        // A rename whose write never landed: the bond says Summit, the device Trailhead.
        control.bondedName = "Summit"
        let transport = MockTransport(control: control)
        let model = MainScreenModel(
            transport: transport,
            nameReconciler: DeviceNameReconciler(
                transport: transport, bondStore: MockBondStore(control: control))
        )
        model.start()
        try await waitFor("launch reconcile") { control.fixtures.config.name == "Summit" }

        // Diverge again, then drop and regain the link: the reconnect edge reconciles once more,
        // with no hot retry in between.
        var fixtures = control.fixtures
        fixtures.config = DeviceConfig(name: "Trailhead")
        control.fixtures = fixtures
        control.connection = .disconnected
        try await waitFor("link down") { model.connection == .disconnected }
        control.connection = .connected
        try await waitFor("reconnect reconcile") { control.fixtures.config.name == "Summit" }
    }
}
