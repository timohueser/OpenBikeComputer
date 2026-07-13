import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// B3 acceptance, host-side: the main-screen model driven through
/// `MockTransport` — the library-first Planned list (#289), the device-first
/// Tracked list, the live device cluster, search, and swipe-delete's data
/// path. The SYNC state machine itself is `RideSyncCoordinatorTests`' beat
/// (#358); syncs here only stage list/reconcile behavior.
@MainActor
final class MainScreenModelTests: XCTestCase {
    /// Sticky holds — terminal within a test, so waits on `.done` / the confirm
    /// line can't race their own expiry timers on a stalled CI runner. The
    /// timers' expiry behavior is `RideSyncCoordinatorTests`' concern, not this
    /// file's.
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
        // What the composition root does for every mock run: the Planned list is
        // library-first, so fixture routes exist as library records (#289).
        if seedLibrary { control.seedLibrary(into: library) }
        let model = MainScreenModel(
            transport: MockTransport(control: control), library: library, syncTiming: timing,
            now: now)
        return (model, control)
    }

    /// A library record the way the import edge builds one (E1 save).
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

    /// A device link scoped to the default fixture device's identity (#769) —
    /// serial from `default.json`, epoch the fixture DTO default. Records
    /// seeded with it behave like uploads committed against the mock device.
    private func mockLink(_ objectID: UInt16) -> DeviceRouteLink {
        DeviceRouteLink(
            serial: "OBC-24-000317", epoch: FixtureSet.defaultStoreEpoch,
            objectID: DeviceObjectID(objectID))
    }

    /// Poll until `condition` holds (the model moves on free-running tasks).
    private func waitFor(
        _ what: String,
        timeout: Duration = .seconds(30),
        _ condition: () -> Bool
    ) async {
        let deadline = ContinuousClock.now.advanced(by: timeout)
        while !condition() {
            if ContinuousClock.now > deadline {
                XCTFail("timed out waiting for \(what)")
                return
            }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    private func startLoaded(_ model: MainScreenModel) async {
        model.start()
        await waitFor("library load") { model.loadState == .loaded }
    }

    /// Load, then pull the device's rides in — the Tracked list is library-first
    /// (#296), so a ride only becomes a row once it's synced. Tests that assert
    /// on ride rows start from here. Waits for a real post-sync marker (never the
    /// pre-sync `.idle`, which would race the async sync task), then for the
    /// progress caption to come down.
    private func startSynced(_ model: MainScreenModel) async {
        await startLoaded(model)
        model.sync.sync()
        await waitFor("sync completes") {
            model.sync.syncState == .done || model.sync.upToDateToastVisible
        }
        // Under sticky timing `.done` never yields to `.idle` in test time —
        // "settled" is the progress caption coming down, which both branches
        // guarantee by the time their state above is visible.
        await waitFor("sync settles") { model.sync.syncProgress == nil }
    }

    // MARK: Stream lifecycle (#356)

    /// The state/battery streams never finish — the loops must hold the model
    /// weakly so it can deallocate with the streams still live. (The model is
    /// app-lifetime today; this pins the convention, not a shipping leak.)
    func testStreamTasksDoNotRetainTheModel() async {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        weak var leaked: MainScreenModel?
        do {
            let model = MainScreenModel(transport: MockTransport(control: control))
            // Let the one-shot tasks (loadTask, identity-after-load) finish —
            // bounded work may hold the model strongly; the stream loops never.
            await startLoaded(model)
            await waitFor("device identity") { model.deviceName == "Trailhead" }
            leaked = model
        }
        // The model's last strong ref is gone; push an event through the still-
        // open streams so a strongly-capturing loop would show up as a live ref.
        control.connection = .outOfRange
        for _ in 0..<10 { await Task.yield() }
        XCTAssertNil(leaked, "the stream loops must hold the model weakly")
    }

    // MARK: Lists + device cluster

    func testLoadPopulatesListsAndIdentityFromFixtures() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)

        XCTAssertEqual(model.routes.count, 5)
        XCTAssertEqual(model.routes.first?.name, "Kettle Moraine Loop")
        // The identity-verified badge (#770): a fixture the device holds proves
        // up once the scope settles (proof needs the connected (serial, epoch) +
        // a matching catalog CRC), so poll rather than assert on the bare load.
        await waitFor("kettle badge proves") { model.isUploaded(RouteID("kettle-moraine-loop")) }
        XCTAssertFalse(model.isUploaded(RouteID("blue-mounds-backroads")))
        // Tracked is library-first (#296): the device's rides aren't rows until
        // they're synced — a plain load leaves the list empty.
        XCTAssertTrue(model.rides.isEmpty)
        model.sync.sync()
        await waitFor("rides synced in") { model.rides.count == 4 }
        XCTAssertEqual(model.rides[1].name, "Sunday Coffee Spin")
        await waitFor("device identity") { model.deviceName == "Trailhead" }
        await waitFor("battery replay") { model.battery == 82 }
        XCTAssertEqual(model.connection, .connected)
        XCTAssertFalse(model.showsDisconnectedBanner)
    }

    func testBatteryNudgeFlowsLive() async {
        let (model, control) = makeModel(.happyPath)
        await startLoaded(model)
        await waitFor("battery replay") { model.battery == 82 }

        control.battery = 55
        await waitFor("battery nudge") { model.battery == 55 }
    }

    func testConnectionChangeDrivesTheBanner() async {
        let (model, control) = makeModel(.happyPath)
        await startLoaded(model)

        control.connection = .outOfRange
        await waitFor("S4 banner") { model.showsDisconnectedBanner }
        control.connection = .connected
        await waitFor("silent reconnect") { !model.showsDisconnectedBanner }
    }

    func testReadErrorThenRetrySucceeds() async {
        let (model, _) = makeModel(.readError)
        model.start()
        await waitFor("S3 failure") { model.loadState == .failed }

        model.reload()   // the one-shot fault is spent — retry succeeds
        await waitFor("retry load") { model.loadState == .loaded }
        XCTAssertEqual(model.routes.count, 5)
    }

    // MARK: Search

    func testSearchFiltersBothTabsCaseInsensitively() async {
        let (model, _) = makeModel(.happyPath)
        await startSynced(model)   // Tracked is library-first: sync so rides exist to filter

        model.searchText = "sugar"
        XCTAssertEqual(model.filteredRoutes.map(\.name), ["Sugar River Trail"])

        model.searchText = "COFFEE"
        XCTAssertEqual(model.filteredRides.map(\.name), ["Sunday Coffee Spin"])
        XCTAssertTrue(model.filteredRoutes.isEmpty)   // H6 on the other tab

        model.searchText = ""
        XCTAssertEqual(model.filteredRoutes.count, 5)
        XCTAssertEqual(model.filteredRides.count, 4)
    }

    // MARK: Sync × the lists (the state machine itself: RideSyncCoordinatorTests)

    /// A ride that syncs in surfaces in the Tracked list this session — the
    /// `onRideLanded` seam feeding the in-memory list per landed ride.
    func testRideAddedOnDeviceSurfacesInTheListAfterSync() async {
        let (model, control) = makeModel(.happyPath)
        await startSynced(model)

        control.emit(.rideAdded(RideSummary(
            id: RideID("ride-new"),
            name: "Lunch Loop",
            date: Date(),
            distanceMeters: 18_000,
            movingTime: 2_800,
            averageSpeedMps: 6.4
        )))

        model.sync.sync()
        await waitFor("one new ride") { model.sync.lastSyncCount == 1 }
        XCTAssertEqual(model.rides.first?.name, "Lunch Loop")
    }

    /// #294 follow-up: a synced ride's full tracklog is available for the
    /// interactive map (never just the ride card's downsampled preview).
    func testRideGeometryIsAvailableAfterSync() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)

        model.sync.sync()
        await waitFor("first sync") { model.sync.lastSyncCount != nil }

        let ride = model.rides.first { $0.name == "Kettle Moraine Loop" }
        let geometry = ride.flatMap { model.rideGeometry(for: $0.id) }
        XCTAssertNotNil(geometry, "a synced ride's points should be available for the map")
        XCTAssertFalse(geometry?.isEmpty ?? true)

        XCTAssertNil(model.rideGeometry(for: RideID("nonexistent")))
    }

    /// H10 at the model level: while a dropped sync waits for Resume, the S4
    /// banner yields to the interruption's — one banner at a time, even though
    /// the link really is out of range. (The drop machinery itself is
    /// coordinator-tested; this pins the model's banner arbitration.)
    func testInterruptionBannerOutranksTheDisconnectedBanner() async {
        let (model, control) = makeModel(.happyPath)
        await startLoaded(model)

        control.dropTransfer(atFraction: 0.5)
        model.sync.sync()
        await waitFor("H10 raised") { model.sync.syncInterruption != nil }

        XCTAssertEqual(model.connection, .outOfRange)
        XCTAssertFalse(model.showsDisconnectedBanner,
                       "the H10 banner tells the link story — never two banners")
    }

    /// Library-first tracked (#296): an un-synced device ride isn't a row at all;
    /// once synced, its row carries the track preview the downloaded payload built
    /// — not the empty §7.4 list summary — so the card/detail never draws the
    /// placeholder glyph for a ride the phone actually holds.
    ///
    /// The default mock `listRides` returns fixture summaries that already carry a
    /// preview (richer than the wire), so this test injects a ride shaped like the
    /// real device's: a **preview-less** list summary over a payload that *does*
    /// have geometry.
    func testSyncedOnDeviceRideShowsATrackPreview() async {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(.happyPath, library: library, seedLibrary: false)
        // Reshape the ride catalog to match the wire: the list summary has no
        // preview (like a real `rideList` entry), but the payload has real points.
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

        await startLoaded(model)

        // Library-first: an un-synced device ride is not a row yet (no half-empty
        // card of stats-without-track).
        XCTAssertNil(model.rides.first { $0.id == RideID("42") },
                     "an un-synced device ride isn't listed")

        model.sync.sync()
        await waitFor("sync done") { model.sync.syncState == .done }

        // Synced → now a row, carrying the preview the downloaded payload built.
        let after = model.rides.first { $0.id == RideID("42") }
        XCTAssertNotNil(after, "the synced ride is now a row")
        XCTAssertFalse(after?.trackPreview?.points.isEmpty ?? true,
                       "a synced ride shows the downloaded track, not the placeholder")
    }

    // MARK: Protocol version (#303)

    /// A device reporting a `protocol_version` this build doesn't speak surfaces
    /// the mismatch (banner state) and disables sync — the app must never proceed
    /// to decode an incompatible object.
    func testProtocolMismatchSurfacesAndDisablesSync() async {
        let (model, control) = makeModel(.happyPath)
        // The device jumps a protocol version ahead of what the app speaks.
        control.deviceInfo = DeviceInfo(
            name: "Trailhead", firmwareVersion: "9.9.9",
            protocolVersion: OBCProtocol.version + 1
        )
        await startLoaded(model)
        await waitFor("mismatch surfaces") { model.protocolMismatch != nil }
        XCTAssertEqual(
            model.protocolMismatch,
            .init(expected: OBCProtocol.version, found: OBCProtocol.version + 1)
        )

        // Disabled sync: pressing Sync must not start a transfer (no decode) —
        // the coordinator asks the model through the injected `canSync` veto.
        model.sync.sync()
        try? await Task.sleep(for: .milliseconds(80))
        XCTAssertEqual(model.sync.syncState, .idle)
        XCTAssertNil(model.sync.syncProgress)
        XCTAssertFalse(model.sync.upToDateToastVisible)
        // A reload after the mismatch is known must not decode the device either.
        model.reload()
        XCTAssertEqual(model.loadState, .loaded)
    }

    /// The matched-version happy path never false-positives.
    func testMatchingProtocolVersionDoesNotFlag() async {
        let (model, _) = makeModel(.happyPath)   // fixtures report OBCProtocol.version
        await startLoaded(model)
        await waitFor("device identity") { model.deviceName == "Trailhead" }
        XCTAssertNil(model.protocolMismatch)
        // Sync still runs on a matched device.
        model.sync.sync()
        await waitFor("sync runs") {
            model.sync.syncState == .syncing || model.sync.lastSyncCount != nil
        }
    }

    // MARK: Delete (H11 → H1)

    func testDeleteRouteRemovesFromLibraryButNeverFromDevice() async {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(.happyPath, library: library)
        await startLoaded(model)

        let id = model.routes[0].id
        model.deleteRoute(id)
        XCTAssertEqual(model.routes.count, 4)   // optimistic removal
        XCTAssertFalse(library.plannedRoutes().contains { $0.id == id }, "delete reaches the library")
        // H1's promise: "If it's already on the device, it stays there."
        XCTAssertTrue(control.fixtures.routes.contains { $0.deviceObjectID != nil })

        model.reload()
        await waitFor("reload") { model.loadState == .loaded }
        XCTAssertFalse(model.routes.contains { $0.id == id }, "the device copy must not re-list it")
    }

    func testDeleteRideMovesToTrashAndStaysOutOfNewCounts() async {
        let library = InMemoryLibraryStore()
        let (model, _) = makeModel(.happyPath, library: library)
        await startSynced(model)
        XCTAssertEqual(model.rides.count, 4)

        let id = model.rides[0].id
        model.deleteRide(id)
        XCTAssertEqual(model.rides.count, 3)
        // #292: delete is a move to Recently Deleted, not a destroy — the
        // stored files stay, so Recover has something to bring back.
        XCTAssertEqual(model.trashedRides.map(\.id), [id])
        XCTAssertNotNil(library.ridePoints(id), "the trashed ride's tracklog must survive")

        // The trashed ride must not come back as a "new" sync count — its id
        // stays marked synced (the coordinator re-reads the library's synced
        // set per sync), so a re-sync finds nothing fresh (H9), and it never
        // re-lists (the device's SD-card copy is untouched by design).
        model.sync.sync()
        await waitFor("re-sync settles") {
            model.sync.upToDateToastVisible || model.sync.syncState == .done
        }
        XCTAssertFalse(model.rides.contains { $0.id == id }, "trashed ride resurrected by sync")
        XCTAssertEqual(model.rides.count, 3)

        model.reload()
        await waitFor("reload") { model.loadState == .loaded }
        XCTAssertFalse(model.rides.contains { $0.id == id }, "trashed ride resurrected by reload")
        XCTAssertEqual(model.rides.count, 3)
    }

    /// The trash persists: a ride deleted on the phone stays out of Tracked
    /// (and in Recently Deleted) across a relaunch, even though the device
    /// still lists it.
    func testTrashedRideStaysTrashedAcrossRelaunch() async {
        let library = InMemoryLibraryStore()
        let (first, _) = makeModel(.happyPath, library: library)
        await startSynced(first)
        let id = first.rides[0].id
        first.deleteRide(id)

        let (relaunched, _) = makeModel(.happyPath, library: library)
        await startLoaded(relaunched)
        XCTAssertFalse(relaunched.rides.contains { $0.id == id })
        XCTAssertEqual(relaunched.trashedRides.map(\.id), [id], "trash lost across relaunch")

        relaunched.sync.sync()
        await waitFor("sync settles") {
            relaunched.sync.syncState == .done || relaunched.sync.upToDateToastVisible
        }
        XCTAssertFalse(relaunched.rides.contains { $0.id == id }, "trash mark lost across relaunch")
    }

    /// Recover puts the ride back in Tracked (in date order, tracklog intact)
    /// — and the recovery itself persists.
    func testRecoverRideRestoresTheRow() async {
        let library = InMemoryLibraryStore()
        let (model, _) = makeModel(.happyPath, library: library)
        await startSynced(model)
        let id = model.rides[1].id
        let countBefore = model.rides.count

        model.deleteRide(id)
        model.recoverRide(id)
        XCTAssertEqual(model.rides.count, countBefore)
        XCTAssertEqual(model.rides[1].id, id, "a recovered ride returns to its date slot")
        XCTAssertTrue(model.trashedRides.isEmpty)
        XCTAssertNotNil(model.rideGeometry(for: id), "the tracklog survived the round trip")

        let (relaunched, _) = makeModel(.happyPath, library: library)
        await startLoaded(relaunched)
        XCTAssertTrue(relaunched.rides.contains { $0.id == id }, "recovery lost across relaunch")
        XCTAssertTrue(relaunched.trashedRides.isEmpty)
    }

    /// Delete Permanently is the old hard delete: files gone, tombstone durable
    /// — a later sync must neither re-download nor re-list the ride.
    func testDeleteRideForeverRemovesFilesAndTombstones() async {
        let library = InMemoryLibraryStore()
        let (model, _) = makeModel(.happyPath, library: library)
        await startSynced(model)
        let id = model.rides[0].id

        model.deleteRide(id)
        model.deleteRideForever(id)
        XCTAssertTrue(model.trashedRides.isEmpty)
        XCTAssertNil(library.ridePoints(id), "the tracklog dies with the permanent delete")

        let (relaunched, _) = makeModel(.happyPath, library: library)
        await startLoaded(relaunched)
        XCTAssertFalse(relaunched.rides.contains { $0.id == id })
        XCTAssertTrue(relaunched.trashedRides.isEmpty)
        relaunched.sync.sync()
        await waitFor("sync settles") {
            relaunched.sync.syncState == .done || relaunched.sync.upToDateToastVisible
        }
        XCTAssertFalse(relaunched.rides.contains { $0.id == id }, "purged ride resurrected by sync")
    }

    /// The retention sweep: a ride trashed longer than `trashRetentionDays`
    /// ago is purged at the next launch; a fresher one stays recoverable.
    func testExpiredTrashIsPurgedAtStart() async {
        let library = InMemoryLibraryStore()
        let (first, _) = makeModel(.happyPath, library: library)
        await startSynced(first)
        let expired = first.rides[0].id
        let fresh = first.rides[1].id
        first.deleteRide(expired)

        // "Relaunch" a day short of the window: still in the trash…
        let almost = Date().addingTimeInterval(
            TimeInterval(MainScreenModel.trashRetentionDays - 1) * 86_400)
        let (kept, _) = makeModel(.happyPath, library: library, now: { almost })
        await startLoaded(kept)
        XCTAssertEqual(kept.trashedRides.map(\.id), [expired])
        // …and this launch trashes the second ride (dated `almost`).
        kept.deleteRide(fresh)

        // …then past it: the first purge runs, the second survives.
        let later = Date().addingTimeInterval(
            TimeInterval(MainScreenModel.trashRetentionDays + 1) * 86_400)
        let (relaunched, _) = makeModel(.happyPath, library: library, now: { later })
        await startLoaded(relaunched)
        XCTAssertEqual(relaunched.trashedRides.map(\.id), [fresh])
        XCTAssertNil(library.ridePoints(expired), "expired trash must be removed for good")
        XCTAssertNotNil(library.ridePoints(fresh))
    }

    // MARK: Rename (H12) + import landing (E1) — session-local edits

    func testRenameUpdatesTheLists() async {
        let (model, _) = makeModel(.happyPath)
        await startSynced(model)

        model.renameRoute(model.routes[0].id, to: "Kettle Gravel Day")
        XCTAssertEqual(model.routes[0].name, "Kettle Gravel Day")

        model.renameRide(model.rides[1].id, to: "Sunday Espresso Spin")
        XCTAssertEqual(model.rides[1].name, "Sunday Espresso Spin")
    }

    func testAddImportedRouteLandsOnTopOfPlannedAndKeepsItsDetail() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)
        model.tab = .tracked

        let record = importedRecord()
        model.addImportedRoute(record)

        XCTAssertEqual(model.routes.count, 6)
        XCTAssertEqual(model.routes[0].id, record.id)
        XCTAssertEqual(model.tab, .planned, "saving lands the user on the Planned list")

        // Reopening must not lose the parsed data; a rename must show in it.
        model.renameRoute(record.id, to: "Schwarzwald Day 2")
        let kept = model.importedDetail(for: record.id)
        XCTAssertEqual(kept?.waypoints.count, 1)
        XCTAssertFalse(kept?.elevationProfile.isEmpty ?? true, "profile derives from the saved geometry")
        XCTAssertEqual(kept?.summary.name, "Schwarzwald Day 2")

        model.deleteRoute(record.id)
        XCTAssertNil(model.importedDetail(for: record.id))
    }

    // MARK: "On device" badge (B13)

    func testUploadCompletionLightsTheOnDeviceBadge() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)
        await waitFor("the identity scope") { model.connectedScope != nil }
        let record = importedRecord()
        model.addImportedRoute(record)
        XCTAssertFalse(model.isUploaded(record.id), "a fresh import isn't on the device")

        // What the upload sheet's onCompleted does with the device-assigned id
        // + the committed payload's fingerprint.
        model.markRouteUploaded(record.id, objectID: DeviceObjectID(7), crc32: RouteObjectCodec.payloadCRC(for: record))
        XCTAssertTrue(model.isUploaded(record.id))
        XCTAssertEqual(model.onDeviceState(record.id), .upToDate)
        XCTAssertEqual(model.plannedDeviceObjectID(for: record.id), DeviceObjectID(7))

        model.deleteRoute(record.id)
        XCTAssertFalse(model.isUploaded(record.id), "deleting clears the badge")
    }

    /// An **on-device** delete while the app is open and connected (epic #447
    /// P6, the Route menu's hold-to-delete): the device notifies `storeChanged`
    /// and the badge clears live — no reconnect, no manual refresh. The mock's
    /// `deviceDeletesRoute` sends exactly the wire sequence (catalog forgets
    /// the copy, then the `storeChanged` edge).
    func testOnDeviceDeleteClearsTheBadgeLive() async {
        let (model, control) = makeModel(.happyPath)
        await startLoaded(model)
        await waitFor("the fixture proves on-device") { model.isUploaded(RouteID("kettle-moraine-loop")) }

        control.deviceDeletesRoute(DeviceObjectID(7)) // kettle-moraine-loop's device copy
        await waitFor("badge clears on storeChanged") { !model.isUploaded(RouteID("kettle-moraine-loop")) }
        // The record survives — only its device link is gone, so a re-upload is offered.
        XCTAssertTrue(model.routes.contains { $0.id == RouteID("kettle-moraine-loop") })
    }

    /// The update lifecycle: an uploaded route is **up to date** (nothing to
    /// push) until its content moves — a rename out-dates it (the name rides
    /// in the payload), and the next committed upload brings it current again
    /// under the same object id.
    func testRenameOutdatesTheDeviceCopyAndReuploadHeals() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)
        await waitFor("the identity scope") { model.connectedScope != nil }
        let record = importedRecord()
        model.addImportedRoute(record)
        model.markRouteUploaded(record.id, objectID: DeviceObjectID(7), crc32: RouteObjectCodec.payloadCRC(for: record))
        XCTAssertEqual(model.onDeviceState(record.id), .upToDate)

        model.renameRoute(record.id, to: "Schwarzwald Tour (final)")
        XCTAssertEqual(model.onDeviceState(record.id), .outdated, "the name rides in the payload")
        XCTAssertEqual(model.plannedDeviceObjectID(for: record.id), DeviceObjectID(7), "the device link survives the rename")

        // The next upload commits the renamed payload — current again.
        var renamed = record
        renamed.summary.name = "Schwarzwald Tour (final)"
        model.markRouteUploaded(record.id, objectID: DeviceObjectID(7), crc32: RouteObjectCodec.payloadCRC(for: renamed))
        XCTAssertEqual(model.onDeviceState(record.id), .upToDate)
    }

    /// V6 (#770): a link with **no committed fingerprint** (a pre-fingerprint
    /// library entry) is unproven — the app can't verify what the linked id
    /// points at, so it shows **no badge**, never a checkmark on presence alone.
    /// The route still offers Upload (not a disabled "up to date"), so the next
    /// push self-heals it with a real fingerprint. (Was `…ReadsAsOutdated` under
    /// v1's presence-lit badge.)
    func testUnknownFingerprintShowsNoBadge() async {
        let library = InMemoryLibraryStore()
        var record = importedRecord()
        record.deviceLink = mockLink(7)   // linked, but `uploadedCRC32` stays nil
        library.savePlannedRoute(record)

        let (model, _) = makeModel(.happyPath, library: library)
        await startLoaded(model)
        await waitFor("the identity scope") { model.connectedScope != nil }
        XCTAssertEqual(model.onDeviceState(record.id), .notOnDevice)
        XCTAssertFalse(model.isUploaded(record.id), "no fingerprint proves nothing — no badge")
    }

    /// The mock-seeded fixtures carry real fingerprints, so a device-held
    /// fixture boots up to date — the C1 checkmark, not the outdated ring.
    func testSeededDeviceCopiesBootUpToDate() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)
        await waitFor("the proven up-to-date badge") {
            model.onDeviceState(RouteID("kettle-moraine-loop")) == .upToDate
        }
    }

    /// A fixture the device holds is seeded with a scoped link + committed
    /// fingerprint (exactly what an upload would mint). A fresh model re-proves
    /// the badge against the catalog CRC — never on link presence alone (#770) —
    /// and threads the object id for replace-by-id, once the identity read
    /// settles on the matching (serial, epoch) (#769).
    func testSeededDeviceCopyKeepsItsProvenBadgeAndReplaceTarget() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)
        await waitFor("the badge proves") { model.isUploaded(RouteID("kettle-moraine-loop")) }
        XCTAssertEqual(model.onDeviceState(RouteID("kettle-moraine-loop")), .upToDate)
        await waitFor("the identity scope") { model.connectedScope != nil }
        XCTAssertEqual(model.plannedDeviceObjectID(for: RouteID("kettle-moraine-loop")), DeviceObjectID(7))
    }

    /// #289's reconcile: a copy deleted out from under us (another phone, the
    /// EchoHarness) clears the stored link — and the badge — on the next reload.
    func testReloadClearsTheBadgeWhenTheDeviceNoLongerHoldsTheRoute() async {
        let library = InMemoryLibraryStore()
        var record = importedRecord()
        record.deviceLink = mockLink(999)   // no fixture device object has this id
        library.savePlannedRoute(record)

        let (model, _) = makeModel(.happyPath, library: library)
        await startLoaded(model)
        // The clear is a reconcile write, so it waits for the identity scope
        // (#769's fail-closed rule) — poll rather than assert immediately.
        await waitFor("the stale link to clear") { !model.isUploaded(record.id) }
        XCTAssertNil(model.plannedDeviceObjectID(for: record.id))
        XCTAssertNil(library.plannedRoutes().first { $0.id == record.id }?.deviceLink,
                     "the cleared link persists")
    }

    /// The full round trip: an upload's committed id keeps the badge lit through
    /// the next reload, because the mock device now lists the copy.
    func testUploadedRouteKeepsItsBadgeThroughReload() async {
        let (model, control) = makeModel(.happyPath)
        await startLoaded(model)
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
        await waitFor("reload") { model.loadState == .loaded }
        XCTAssertTrue(model.isUploaded(record.id), "the device lists the fresh copy — the badge survives reconcile")
    }

    /// A regained link re-reads the lists by itself: the device may have changed
    /// while the app was away, so the badge must true-up on reconnect without a
    /// manual reload.
    func testReconnectReloadsAndReconcilesTheBadge() async {
        let (model, control) = makeModel(.happyPath)
        await startLoaded(model)
        await waitFor("the fixture proves on-device") { model.isUploaded(RouteID("kettle-moraine-loop")) }

        // The device loses the copy (another phone / the EchoHarness deleted
        // object 7); nothing tells the model — the badge stays lit for now.
        try? await MockTransport(control: control).deleteRoute(DeviceObjectID(7))
        XCTAssertTrue(model.isUploaded(RouteID("kettle-moraine-loop")))

        control.connection = .outOfRange
        await waitFor("S4 banner") { model.showsDisconnectedBanner }
        control.connection = .connected
        await waitFor("reconnect reconcile") { !model.isUploaded(RouteID("kettle-moraine-loop")) }
    }

    // MARK: V6 — identity-verified badges + adopt-by-content (#770)

    /// CRC-mismatch drops the link: a link that survived scoping but points at an
    /// object the device has since **replaced** (era aliasing, or another phone's
    /// upload) — the catalog CRC disagrees with our committed fingerprint — is
    /// dropped, never shown "up to date" on presence (#770).
    func testCRCMismatchDropsTheLinkNeverACheckmark() async {
        let library = InMemoryLibraryStore()
        var record = importedRecord(id: "lib-mismatch")
        record.deviceLink = mockLink(7)        // object 7 exists (the kettle copy)…
        record.uploadedCRC32 = 0xDEAD_BEEF     // …but the device doesn't hold this
        library.savePlannedRoute(record)

        let (model, _) = makeModel(.happyPath, library: library)
        await startLoaded(model)
        await waitFor("the mismatched link drops") {
            library.plannedRoutes().first { $0.id == record.id }?.deviceLink == nil
        }
        XCTAssertEqual(model.onDeviceState(record.id), .notOnDevice)
        XCTAssertNil(model.plannedDeviceObjectID(for: record.id),
                     "a mismatched link must not thread a replace target (the wrong-route-overwrite bug)")
    }

    /// Adopt-by-content heals an app reinstall: the library kept the route but
    /// lost its device link; the device still holds an identical copy → adoption
    /// re-links it (badge lights, no re-upload), and a later push replaces that
    /// object by id instead of duplicating (#770).
    func testAdoptByContentHealsAnAppReinstall() async {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(.happyPath, library: library, seedLibrary: false)
        // The device holds this content under object 900; the phone kept the
        // route but (post-reinstall) has no link to it.
        let template = importedRecord(id: "lib-adopt", name: "Reinstalled Ridge")
        var fixtures = control.fixtures
        fixtures.routes.append(RouteEntry(
            summary: template.summary, points: template.route.points,
            waypoints: template.route.waypoints, payloadByteCount: 100,
            deviceObjectID: DeviceObjectID(900)))
        control.fixtures = fixtures
        library.savePlannedRoute(template)   // no deviceLink

        await startLoaded(model)
        await waitFor("adoption re-links the identical copy") {
            library.plannedRoutes().first { $0.id == template.id }?.deviceLink != nil
        }
        await waitFor("the identity scope") { model.connectedScope != nil }
        XCTAssertEqual(model.plannedDeviceObjectID(for: template.id), DeviceObjectID(900),
                       "the adopted id threads replace-by-id")
        XCTAssertEqual(model.onDeviceState(template.id), .upToDate,
                       "adopted content is byte-identical → up to date, no upload needed")
    }

    /// Adopted-upload-replaces: after adoption an edit re-uploads to the adopted
    /// object id (replace-by-id), so the device gains no duplicate (#770).
    func testAdoptedRouteUploadsAsReplaceNotDuplicate() async {
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

        await startLoaded(model)
        await waitFor("adoption") { model.plannedDeviceObjectID(for: template.id) == DeviceObjectID(901) }
        let deviceRouteCountBefore = control.fixtures.routes.filter { $0.deviceObjectID != nil }.count

        // The upload the app would send (targeting the adopted id) replaces in
        // place — no 0xFFFF-new duplicate.
        let blob = RouteBlob(
            summary: template.summary, waypoints: template.route.waypoints,
            payload: Data([9, 9, 9]), targetObjectID: model.plannedDeviceObjectID(for: template.id))
        let handle = MockTransport(control: control).uploadRoute(blob)
        _ = await handle.outcome
        XCTAssertEqual(
            control.fixtures.routes.filter { $0.deviceObjectID != nil }.count,
            deviceRouteCountBefore, "an adopted upload replaces by id — never a duplicate")
    }

    /// Unknown-CRC conservatism: a device whose route sidecar hasn't filled yet
    /// reports `crc32 = 0`. `0` proves nothing → no badge; but it's not a
    /// *disproof* either, so the link is kept, not dropped (#770).
    func testUnknownCatalogCRCProvesNothingButKeepsTheLink() async {
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

        await startLoaded(model)
        await waitFor("the identity scope") { model.connectedScope != nil }
        XCTAssertEqual(model.onDeviceState(record.id), .notOnDevice, "crc32 = 0 proves nothing — no badge")
        try? await Task.sleep(for: .milliseconds(50))   // let any reconcile write land
        XCTAssertNotNil(library.plannedRoutes().first { $0.id == record.id }?.deviceLink,
                        "an unknown CRC is not a disproof — the link is kept")
    }

    /// Per-serial isolation: a link minted on another device (serial B) is
    /// invisible and untouchable while connected to device A — no badge, and A's
    /// catalog can't clear it (V5's predicate makes this structural; pin it).
    func testDeviceBCatalogNeverTouchesDeviceALinks() async {
        let library = InMemoryLibraryStore()
        let foreignLink = DeviceRouteLink(
            serial: "OBC-OTHER-DEVICE", epoch: FixtureSet.defaultStoreEpoch, objectID: DeviceObjectID(7))
        var record = importedRecord(id: "lib-deviceB")
        record.deviceLink = foreignLink
        record.uploadedCRC32 = 0xAAAA_BBBB
        library.savePlannedRoute(record)

        let (model, _) = makeModel(.happyPath, library: library)   // connects to device A
        await startLoaded(model)
        await waitFor("the identity scope") { model.connectedScope != nil }
        XCTAssertEqual(model.onDeviceState(record.id), .notOnDevice, "device B's link never badges on device A")
        XCTAssertNil(model.plannedDeviceObjectID(for: record.id), "…and never threads a replace target")
        try? await Task.sleep(for: .milliseconds(50))
        XCTAssertEqual(
            library.plannedRoutes().first { $0.id == record.id }?.deviceLink, foreignLink,
            "device A's reconcile must not clear device B's link")
    }

    func testPlannedRouteNamedFindsACollisionCaseInsensitively() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)
        model.addImportedRoute(importedRecord(name: "Schwarzwald Tour · Tag 2"))
        XCTAssertNotNil(model.plannedRoute(named: "schwarzwald tour · tag 2"))
        XCTAssertNil(model.plannedRoute(named: "A Different Route"))
    }

    // MARK: The library store (B1S) — persistence across "relaunches"

    /// #256 acceptance: an H4 import saved before/without a device survives a
    /// relaunch (a second model over the same store), rename included.
    func testImportedRouteSurvivesRelaunch() async {
        let library = InMemoryLibraryStore()
        let (first, _) = makeModel(.happyPath, library: library)
        await startLoaded(first)
        let record = importedRecord()
        first.addImportedRoute(record)
        first.renameRoute(record.id, to: "Schwarzwald Day 2")

        let (relaunched, _) = makeModel(.happyPath, library: library)
        await startLoaded(relaunched)

        XCTAssertEqual(relaunched.routes.count, 6, "the saved import joins the seeded five")
        XCTAssertEqual(relaunched.routes.first?.id, record.id, "the newest save stays on top")
        XCTAssertEqual(relaunched.routes.first?.name, "Schwarzwald Day 2")
        let kept = relaunched.importedDetail(for: record.id)
        XCTAssertEqual(kept?.waypoints.count, 1, "the parsed detail is derivable after relaunch")

        relaunched.deleteRoute(record.id)
        XCTAssertFalse(library.plannedRoutes().contains { $0.id == record.id }, "delete reaches the library")
    }

    /// The offline rule: the store is why a failed device read degrades to
    /// browsable content instead of an empty error screen (S4/S3).
    func testStoreSeededListsStayBrowsableWhenTheReadFails() async {
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
        await waitFor("read failure") { model.loadState == .failed }

        XCTAssertEqual(model.routes.map(\.id), [record.id], "planned stays browsable")
        XCTAssertEqual(model.rides.map(\.id), [ride.id], "tracked stays browsable")
    }

    // MARK: Desired-name reconcile (#361)

    /// The once-per-connect trigger: the launch connection reconciles after
    /// the first load, and every regained link reconciles again — a rename
    /// whose config write never landed converges without any user action.
    func testConnectRunsTheDesiredNameReconcile() async {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        // A rename whose write never landed: bond says Summit, device Trailhead.
        control.bondedName = "Summit"
        let transport = MockTransport(control: control)
        let model = MainScreenModel(
            transport: transport,
            nameReconciler: DeviceNameReconciler(
                transport: transport, bondStore: MockBondStore(control: control))
        )
        model.start()
        await waitFor("launch reconcile") { control.fixtures.config.name == "Summit" }

        // Diverge again (the renamed-from-another-phone edge — last-writer-
        // wins) and drop/regain the link: the reconnect edge reconciles once
        // more, no hot retry in between.
        var fixtures = control.fixtures
        fixtures.config = DeviceConfig(name: "Trailhead")
        control.fixtures = fixtures
        control.connection = .disconnected
        await waitFor("link down") { model.connection == .disconnected }
        control.connection = .connected
        await waitFor("reconnect reconcile") { control.fixtures.config.name == "Summit" }
    }
}
