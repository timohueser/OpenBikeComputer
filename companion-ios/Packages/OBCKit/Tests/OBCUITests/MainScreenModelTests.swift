import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The main-screen model driven through `MockTransport`: the library-first
/// Planned list, the device-first Tracked list, the live device cluster,
/// search, the SYNC button state machine, and swipe-delete's data path.
@MainActor
final class MainScreenModelTests: XCTestCase {
    /// Short pacing so the done-hold / line-hold timers fire in test time.
    private static let fastTiming = MainScreenModel.Timing(
        syncDoneHold: .milliseconds(60),
        syncedLineHold: .milliseconds(500)
    )

    private func makeModel(
        _ scenario: Scenario,
        library: any LibraryStore = InMemoryLibraryStore(),
        seedLibrary: Bool = true,
        timing: MainScreenModel.Timing = fastTiming
    ) -> (MainScreenModel, MockControl) {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        // Fast transfers: the pacing under test is the model's, not the mock's.
        control.throughputBytesPerSec = 200_000_000
        // The Planned list is library-first, so fixture routes must exist as
        // library records.
        if seedLibrary { control.seedLibrary(into: library) }
        let model = MainScreenModel(
            transport: MockTransport(control: control), library: library, timing: timing)
        return (model, control)
    }

    /// A library record shaped the way the import edge builds one.
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

    /// Poll until `condition` holds (the model moves on free-running tasks).
    private func waitFor(
        _ what: String,
        timeout: Duration = .seconds(5),
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

    /// Load, then pull the device's rides in — the Tracked list is library-first,
    /// so a ride only becomes a row once it's synced. Tests that assert on ride
    /// rows start from here. Waits for a real post-sync marker (never the
    /// pre-sync `.idle`, which would race the async sync task), then for it to
    /// settle back to idle.
    private func startSynced(_ model: MainScreenModel) async {
        await startLoaded(model)
        model.sync()
        await waitFor("sync completes") { model.syncState == .done || model.upToDateToastVisible }
        await waitFor("sync settles") { model.syncState == .idle && model.syncProgress == nil }
    }

    // MARK: Lists + device cluster

    func testLoadPopulatesListsAndIdentityFromFixtures() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)

        XCTAssertEqual(model.routes.count, 5)
        XCTAssertEqual(model.routes.first?.name, "Kettle Moraine Loop")
        // The badge reconcile: fixtures the device holds a copy of light up.
        XCTAssertTrue(model.isUploaded(RouteID("kettle-moraine-loop")))
        XCTAssertFalse(model.isUploaded(RouteID("blue-mounds-backroads")))
        // Tracked is library-first: the device's rides aren't rows until
        // they're synced — a plain load leaves the list empty.
        XCTAssertTrue(model.rides.isEmpty)
        model.sync()
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
        XCTAssertTrue(model.filteredRoutes.isEmpty)   // filter applies to the other tab too

        model.searchText = ""
        XCTAssertEqual(model.filteredRoutes.count, 5)
        XCTAssertEqual(model.filteredRides.count, 4)
    }

    // MARK: Sync

    func testFirstSyncPullsEverythingThenIdles() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)

        model.sync()
        await waitFor("syncing") { model.syncState == .syncing }
        await waitFor("done + confirm line") {
            model.syncState == .done && model.lastSyncCount == 4
        }
        XCTAssertNil(model.syncProgress)
        await waitFor("done hold expires") { model.syncState == .idle }
        XCTAssertEqual(model.lastSyncCount, 4)   // the line outlives the check
        await waitFor("confirm line expires") { model.lastSyncCount == nil }
    }

    func testSecondSyncIsUpToDate() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)

        model.sync()
        await waitFor("first sync done") { model.lastSyncCount == 4 }
        await waitFor("idle again") { model.syncState == .idle }

        model.sync()
        // Quiet toast, straight back to idle — never an empty "done".
        await waitFor("up-to-date toast") { model.upToDateToastVisible }
        XCTAssertEqual(model.syncState, .idle)
        XCTAssertNil(model.lastSyncCount)
    }

    func testRideAddedOnDeviceSyncsAsOneNewRide() async {
        let (model, control) = makeModel(.happyPath)
        await startLoaded(model)

        model.sync()
        await waitFor("first sync") { model.lastSyncCount == 4 }
        await waitFor("idle") { model.syncState == .idle }

        control.emit(.rideAdded(RideSummary(
            id: RideID("ride-new"),
            name: "Lunch Loop",
            date: Date(),
            distanceMeters: 18_000,
            movingTime: 2_800,
            averageSpeedMps: 6.4
        )))

        model.sync()
        await waitFor("one new ride") { model.lastSyncCount == 1 }
        XCTAssertEqual(model.rides.first?.name, "Lunch Loop")
    }

    /// A synced ride's full tracklog is available for the interactive map —
    /// never just the ride card's downsampled preview.
    func testRideGeometryIsAvailableAfterSync() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)

        model.sync()
        await waitFor("first sync") { model.lastSyncCount != nil }

        let ride = model.rides.first { $0.name == "Kettle Moraine Loop" }
        let geometry = ride.flatMap { model.rideGeometry(for: $0.id) }
        XCTAssertNotNil(geometry, "a synced ride's points should be available for the map")
        XCTAssertFalse(geometry?.isEmpty ?? true)

        XCTAssertNil(model.rideGeometry(for: RideID("nonexistent")))
    }

    /// A drop mid-sync freezes what landed into the banner state — button
    /// idle, progress down, and the disconnect banner yields to the
    /// interruption's.
    func testDropMidSyncRaisesH10WithTheLandedCounts() async {
        let (model, control) = makeModel(.happyPath)
        await startLoaded(model)

        control.dropTransfer(atFraction: 0.5)
        model.sync()
        await waitFor("H10 raised") { model.syncInterruption != nil }
        XCTAssertEqual(model.syncState, .idle)
        XCTAssertNil(model.syncProgress)
        XCTAssertNil(model.lastSyncCount)

        let interruption = model.syncInterruption
        XCTAssertEqual(interruption?.total, 4)
        XCTAssertGreaterThan(interruption?.landed ?? -1, 0, "half the bytes should land some rides")
        XCTAssertLessThan(interruption?.landed ?? 4, 4, "a drop mid-batch can't have landed them all")

        XCTAssertEqual(model.connection, .outOfRange)
        XCTAssertFalse(model.showsDisconnectedBanner,
                       "the H10 banner tells the link story — never two banners")
    }

    /// Resume: the same transfer continues from its last committed offset
    /// and finishes; every ride of the batch counts once.
    func testResumeContinuesTheDroppedSyncToCompletion() async {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(.happyPath, library: library)
        await startLoaded(model)

        control.dropTransfer(atFraction: 0.5)
        model.sync()
        await waitFor("H10 raised") { model.syncInterruption != nil }
        let landedAtDrop = model.syncInterruption?.landed ?? 0

        model.resumeSync()
        XCTAssertNil(model.syncInterruption, "Resume takes the banner down at once")
        XCTAssertEqual(model.syncState, .syncing)
        XCTAssertEqual(model.syncProgress,
                       .init(done: landedAtDrop, total: 4),
                       "the caption picks up where the drop left it")

        await waitFor("batch completes") {
            model.syncState == .done && model.lastSyncCount == 4
        }
        XCTAssertEqual(model.connection, .connected, "resume restores the link")
        XCTAssertEqual(library.syncedRideIDs().count, 4)
        XCTAssertEqual(library.rides().count, 4, "resumed rides persist like the rest")
    }

    /// The user can also just sync again once back in range — what landed
    /// stays synced, so the fresh batch is exactly the remainder.
    func testFreshSyncAfterADropPullsOnlyTheRemainder() async {
        let (model, control) = makeModel(.happyPath)
        await startLoaded(model)

        control.dropTransfer(atFraction: 0.5)
        model.sync()
        await waitFor("H10 raised") { model.syncInterruption != nil }

        control.connection = .connected
        await waitFor("reconnect reaches the model") { model.connection == .connected }
        model.sync()
        XCTAssertNil(model.syncInterruption, "a fresh sync clears the waiting banner")
        await waitFor("remainder synced") {
            model.syncState == .done && model.lastSyncCount != nil
        }
        let remainder = model.lastSyncCount ?? 0
        XCTAssertGreaterThan(remainder, 0, "the fresh sync should find the un-landed rides")
        XCTAssertLessThan(remainder, 4, "partial rides must not be re-counted")
    }

    /// A synced ride lands in the library with its tracklog decoded from the
    /// payload — not as an empty-points summary shell.
    func testSyncedRideCarriesTheDecodedTracklog() async {
        let library = InMemoryLibraryStore()
        let (model, _) = makeModel(.happyPath, library: library)
        await startLoaded(model)

        model.sync()
        await waitFor("sync done") { model.syncState == .done }

        let stored = library.rides()
        XCTAssertEqual(stored.count, 4)
        XCTAssertTrue(stored.allSatisfy { !$0.points.isEmpty },
                      "every fixture payload decodes into a tracklog")

        let kettle = stored.first { $0.id == RideID("ride-kettle-moraine") }
        XCTAssertEqual(kettle?.points.count, 9, "the fixture's track survives the wire")
        let start = kettle?.points.first
        XCTAssertEqual(start?.coordinate.latitude ?? 0, 42.8672, accuracy: 1e-6)
        XCTAssertEqual(start?.coordinate.longitude ?? 0, -88.4471, accuracy: 1e-6)
        XCTAssertEqual(start?.elevationMeters ?? 0, 264, accuracy: 0.5)
        // Timestamps synthesized across the moving time, in ride order.
        let span = kettle.map { $0.points.last!.timestamp.timeIntervalSince($0.points.first!.timestamp) }
        XCTAssertEqual(span ?? 0, 10_260, accuracy: 1)
    }

    /// Library-first tracked: an un-synced device ride isn't a row at all;
    /// once synced, its row carries the track preview the downloaded payload
    /// built — not the empty list summary — so the card/detail never draws
    /// the placeholder glyph for a ride the phone actually holds.
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
                trackPreview: nil  // the list carries no geometry
            ),
            points: ridePoints)]
        control.fixtures = fixtures

        await startLoaded(model)

        // Library-first: an un-synced device ride is not a row yet (no half-empty
        // card of stats-without-track).
        XCTAssertNil(model.rides.first { $0.id == RideID("42") },
                     "an un-synced device ride isn't listed")

        model.sync()
        await waitFor("sync done") { model.syncState == .done }

        // Synced → now a row, carrying the preview the downloaded payload built.
        let after = model.rides.first { $0.id == RideID("42") }
        XCTAssertNotNil(after, "the synced ride is now a row")
        XCTAssertFalse(after?.trackPreview?.points.isEmpty ?? true,
                       "a synced ride shows the downloaded track, not the placeholder")
    }

    func testSyncNoOpsWhenUnreachable() async {
        let (model, _) = makeModel(.outOfRange)
        await startLoaded(model)   // out of range still serves cached fixtures

        model.sync()
        try? await Task.sleep(for: .milliseconds(80))
        XCTAssertEqual(model.syncState, .idle)
        XCTAssertFalse(model.upToDateToastVisible)
    }

    // MARK: Protocol version

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

        // Disabled sync: pressing Sync must not start a transfer (no decode).
        model.sync()
        try? await Task.sleep(for: .milliseconds(80))
        XCTAssertEqual(model.syncState, .idle)
        XCTAssertNil(model.syncProgress)
        XCTAssertFalse(model.upToDateToastVisible)
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
        model.sync()
        await waitFor("sync runs") { model.syncState == .syncing || model.lastSyncCount != nil }
    }

    // MARK: Delete

    func testDeleteRouteRemovesFromLibraryButNeverFromDevice() async {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(.happyPath, library: library)
        await startLoaded(model)

        let id = model.routes[0].id
        model.deleteRoute(id)
        XCTAssertEqual(model.routes.count, 4)   // optimistic removal
        XCTAssertFalse(library.plannedRoutes().contains { $0.id == id }, "delete reaches the library")
        // If it's already on the device, it stays there.
        XCTAssertTrue(control.fixtures.routes.contains { $0.deviceObjectID != nil })

        model.reload()
        await waitFor("reload") { model.loadState == .loaded }
        XCTAssertFalse(model.routes.contains { $0.id == id }, "the device copy must not re-list it")
    }

    func testDeleteRideRemovesLocallyAndStaysOutOfNewCounts() async {
        let (model, _) = makeModel(.happyPath)
        await startSynced(model)
        XCTAssertEqual(model.rides.count, 4)

        let id = model.rides[0].id
        model.deleteRide(id)
        XCTAssertEqual(model.rides.count, 3)

        // The deleted ride must not come back as a "new" sync count — its id
        // stays marked synced, so a re-sync finds nothing fresh, and it
        // never re-lists (the device's SD-card copy is untouched by design).
        model.sync()
        await waitFor("re-sync settles") { model.upToDateToastVisible || model.syncState == .done }
        XCTAssertFalse(model.rides.contains { $0.id == id }, "deleted ride resurrected by sync")
        XCTAssertEqual(model.rides.count, 3)

        model.reload()
        await waitFor("reload") { model.loadState == .loaded }
        XCTAssertFalse(model.rides.contains { $0.id == id }, "deleted ride resurrected by reload")
        XCTAssertEqual(model.rides.count, 3)
    }

    /// The tombstone persists: a ride deleted on the phone stays gone across a
    /// relaunch even though the device still lists it.
    func testDeletedRideStaysGoneAcrossRelaunch() async {
        let library = InMemoryLibraryStore()
        let (first, _) = makeModel(.happyPath, library: library)
        await startSynced(first)
        let id = first.rides[0].id
        first.deleteRide(id)

        let (relaunched, _) = makeModel(.happyPath, library: library)
        await startLoaded(relaunched)
        XCTAssertFalse(relaunched.rides.contains { $0.id == id })

        relaunched.sync()
        await waitFor("sync settles") {
            relaunched.syncState == .done || relaunched.upToDateToastVisible
        }
        XCTAssertFalse(relaunched.rides.contains { $0.id == id }, "tombstone lost across relaunch")
    }

    // MARK: Rename + import landing — session-local edits

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

    // MARK: "On device" badge

    func testUploadCompletionLightsTheOnDeviceBadge() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)
        let record = importedRecord()
        model.addImportedRoute(record)
        XCTAssertFalse(model.isUploaded(record.id), "a fresh import isn't on the device")

        // What the upload sheet's onCompleted does with the device-assigned id
        // + the committed payload's fingerprint.
        model.markRouteUploaded(record.id, objectID: 7, crc32: RouteObjectCodec.payloadCRC(for: record))
        XCTAssertTrue(model.isUploaded(record.id))
        XCTAssertEqual(model.onDeviceState(record.id), .upToDate)
        XCTAssertEqual(model.plannedDeviceObjectID(for: record.id), 7)

        model.deleteRoute(record.id)
        XCTAssertFalse(model.isUploaded(record.id), "deleting clears the badge")
    }

    /// The update lifecycle: an uploaded route is **up to date** (nothing to
    /// push) until its content moves — a rename out-dates it (the name rides
    /// in the payload), and the next committed upload brings it current again
    /// under the same object id.
    func testRenameOutdatesTheDeviceCopyAndReuploadHeals() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)
        let record = importedRecord()
        model.addImportedRoute(record)
        model.markRouteUploaded(record.id, objectID: 7, crc32: RouteObjectCodec.payloadCRC(for: record))
        XCTAssertEqual(model.onDeviceState(record.id), .upToDate)

        model.renameRoute(record.id, to: "Schwarzwald Tour (final)")
        XCTAssertEqual(model.onDeviceState(record.id), .outdated, "the name rides in the payload")
        XCTAssertEqual(model.plannedDeviceObjectID(for: record.id), 7, "the device link survives the rename")

        // The next upload commits the renamed payload — current again.
        var renamed = record
        renamed.summary.name = "Schwarzwald Tour (final)"
        model.markRouteUploaded(record.id, objectID: 7, crc32: RouteObjectCodec.payloadCRC(for: renamed))
        XCTAssertEqual(model.onDeviceState(record.id), .upToDate)
    }

    /// A device copy with an unknown fingerprint (a pre-fingerprint library)
    /// reads as **out of date** — Update stays offered and the next upload
    /// self-heals the record; calling it up to date would dead-end the route
    /// on a disabled button.
    func testUnknownFingerprintReadsAsOutdated() async {
        let library = InMemoryLibraryStore()
        var record = importedRecord()
        record.deviceObjectID = 7
        library.savePlannedRoute(record)

        let (model, _) = makeModel(.happyPath, library: library)
        await startLoaded(model)
        XCTAssertEqual(model.onDeviceState(record.id), .outdated)
        XCTAssertTrue(model.isUploaded(record.id), "the badge still shows — the copy exists")
    }

    /// The mock-seeded fixtures carry real fingerprints, so a device-held
    /// fixture boots up to date — the checkmark, not the outdated ring.
    func testSeededDeviceCopiesBootUpToDate() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)
        XCTAssertEqual(model.onDeviceState(RouteID("kettle-moraine-loop")), .upToDate)
    }

    func testSeededUploadedRouteKeepsItsBadgeWhenTheDeviceStillHoldsIt() async {
        let library = InMemoryLibraryStore()
        var record = importedRecord()
        record.deviceObjectID = 7   // the default fixture device holds object 7
        library.savePlannedRoute(record)

        let (model, _) = makeModel(.happyPath, library: library)
        await startLoaded(model)
        XCTAssertTrue(model.isUploaded(record.id), "a route on the device keeps its badge across a relaunch")
        XCTAssertEqual(model.plannedDeviceObjectID(for: record.id), 7)
    }

    /// A copy deleted out from under us (another phone, the EchoHarness)
    /// clears the stored link — and the badge — on the next reload.
    func testReloadClearsTheBadgeWhenTheDeviceNoLongerHoldsTheRoute() async {
        let library = InMemoryLibraryStore()
        var record = importedRecord()
        record.deviceObjectID = 999   // no fixture device object has this id
        library.savePlannedRoute(record)

        let (model, _) = makeModel(.happyPath, library: library)
        await startLoaded(model)
        XCTAssertFalse(model.isUploaded(record.id))
        XCTAssertNil(model.plannedDeviceObjectID(for: record.id))
        XCTAssertNil(library.plannedRoutes().first { $0.id == record.id }?.deviceObjectID,
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
        XCTAssertTrue(model.isUploaded(RouteID("kettle-moraine-loop")))

        // The device loses the copy (another phone / the EchoHarness deleted
        // object 7); nothing tells the model — the badge stays lit for now.
        try? await MockTransport(control: control).deleteRoute(RouteID("7"))
        XCTAssertTrue(model.isUploaded(RouteID("kettle-moraine-loop")))

        control.connection = .outOfRange
        await waitFor("S4 banner") { model.showsDisconnectedBanner }
        control.connection = .connected
        await waitFor("reconnect reconcile") { !model.isUploaded(RouteID("kettle-moraine-loop")) }
    }

    func testPlannedRouteNamedFindsACollisionCaseInsensitively() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)
        model.addImportedRoute(importedRecord(name: "Schwarzwald Tour · Tag 2"))
        XCTAssertNotNil(model.plannedRoute(named: "schwarzwald tour · tag 2"))
        XCTAssertNil(model.plannedRoute(named: "A Different Route"))
    }

    // MARK: The library store — persistence across "relaunches"

    /// An import saved before/without a device survives a relaunch (a second
    /// model over the same store), rename included.
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

    /// Re-sync after a relaunch downloads nothing new.
    func testResyncAfterRelaunchIsUpToDate() async {
        let library = InMemoryLibraryStore()
        let (first, _) = makeModel(.happyPath, library: library)
        await startLoaded(first)
        first.sync()
        await waitFor("first sync") { first.lastSyncCount == 4 }
        XCTAssertEqual(library.rides().count, 4, "each landed ride persists")

        let (relaunched, _) = makeModel(.happyPath, library: library)
        await startLoaded(relaunched)
        relaunched.sync()

        await waitFor("H9 across the relaunch") { relaunched.upToDateToastVisible }
        XCTAssertEqual(relaunched.syncState, .idle)
        XCTAssertNil(relaunched.lastSyncCount)
    }

    /// A sync interrupted at N of M keeps the N across a relaunch — the next
    /// sync pulls only the remainder.
    func testPartialSyncSurvivesRelaunch() async {
        let library = InMemoryLibraryStore()
        let (first, control) = makeModel(.happyPath, library: library)
        await startLoaded(first)
        control.dropTransfer(atFraction: 0.5)
        first.sync()
        await waitFor("drop observed") { first.connection == .outOfRange }
        await waitFor("back to idle") { first.syncState == .idle }

        let landed = library.syncedRideIDs().count
        XCTAssertTrue((1...3).contains(landed), "the drop should leave a partial batch")
        XCTAssertEqual(library.rides().count, landed, "what landed is already persisted")

        let (relaunched, _) = makeModel(.happyPath, library: library)
        await startLoaded(relaunched)
        relaunched.sync()
        await waitFor("remainder synced") { relaunched.syncState == .done }
        XCTAssertEqual(relaunched.lastSyncCount, 4 - landed)
    }

    /// The offline rule: the store is why a failed device read degrades to
    /// browsable content instead of an empty error screen.
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
}
