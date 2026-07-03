import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// B3 acceptance, host-side: the main-screen model driven through
/// `MockTransport` — the library-first Planned list (#289), the device-first
/// Tracked list, the live device cluster, search, the SYNC button state
/// machine, and swipe-delete's data path.
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
        // What the composition root does for every mock run: the Planned list is
        // library-first, so fixture routes exist as library records (#289).
        if seedLibrary { control.seedLibrary(into: library) }
        let model = MainScreenModel(
            transport: MockTransport(control: control), library: library, timing: timing)
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

    // MARK: Lists + device cluster

    func testLoadPopulatesListsAndIdentityFromFixtures() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)

        XCTAssertEqual(model.routes.count, 5)
        XCTAssertEqual(model.routes.first?.name, "Kettle Moraine Loop")
        // The badge reconcile: fixtures the device holds a copy of light up.
        XCTAssertTrue(model.isUploaded(RouteID("kettle-moraine-loop")))
        XCTAssertFalse(model.isUploaded(RouteID("blue-mounds-backroads")))
        XCTAssertEqual(model.rides.count, 4)
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
        await startLoaded(model)

        model.searchText = "sugar"
        XCTAssertEqual(model.filteredRoutes.map(\.name), ["Sugar River Trail"])

        model.searchText = "COFFEE"
        XCTAssertEqual(model.filteredRides.map(\.name), ["Sunday Coffee Spin"])
        XCTAssertTrue(model.filteredRoutes.isEmpty)   // H6 on the other tab

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
        // H9: quiet toast, straight back to idle — never an empty "done".
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

    /// H10: the drop freezes what landed into the banner state — button idle,
    /// progress down, and the S4 banner yields to the interruption's.
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

    /// H10 → Resume: the same transfer continues from its last committed
    /// offset and finishes; every ride of the batch counts once.
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

    /// B7's decode path: a synced ride lands in the library with its tracklog
    /// decoded from the payload — not as an empty-points summary shell.
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

    func testSyncNoOpsWhenUnreachable() async {
        let (model, _) = makeModel(.outOfRange)
        await startLoaded(model)   // out of range still serves cached fixtures

        model.sync()
        try? await Task.sleep(for: .milliseconds(80))
        XCTAssertEqual(model.syncState, .idle)
        XCTAssertFalse(model.upToDateToastVisible)
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

    func testDeleteRideRemovesLocallyAndStaysOutOfNewCounts() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)

        let id = model.rides[0].id
        model.deleteRide(id)
        XCTAssertEqual(model.rides.count, 3)

        // The deleted ride must not come back as a "new" sync count — and the
        // sync's list merge must not resurrect it (the device still lists it;
        // its SD-card copy is untouched by design).
        model.sync()
        await waitFor("sync done") { model.syncState == .done }
        XCTAssertEqual(model.lastSyncCount, 3)
        XCTAssertFalse(model.rides.contains { $0.id == id }, "deleted ride resurrected by sync")
        XCTAssertEqual(model.rides.count, 3)

        model.reload()
        await waitFor("reload") { model.loadState == .loaded }
        XCTAssertFalse(model.rides.contains { $0.id == id }, "deleted ride resurrected by reload")
    }

    /// The tombstone persists: a ride deleted on the phone stays gone across a
    /// relaunch even though the device still lists it.
    func testDeletedRideStaysGoneAcrossRelaunch() async {
        let library = InMemoryLibraryStore()
        let (first, _) = makeModel(.happyPath, library: library)
        await startLoaded(first)
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

    // MARK: Rename (H12) + import landing (E1) — session-local edits

    func testRenameUpdatesTheLists() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)

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
        let record = importedRecord()
        model.addImportedRoute(record)
        XCTAssertFalse(model.isUploaded(record.id), "a fresh import isn't on the device")

        // What the upload sheet's onCompleted does with the device-assigned id.
        model.markRouteUploaded(record.id, objectID: 7)
        XCTAssertTrue(model.isUploaded(record.id))
        XCTAssertEqual(model.plannedDeviceObjectID(for: record.id), 7)

        model.deleteRoute(record.id)
        XCTAssertFalse(model.isUploaded(record.id), "deleting clears the badge")
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

    /// #289's reconcile: a copy deleted out from under us (another phone, the
    /// EchoHarness) clears the stored link — and the badge — on the next reload.
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
        model.markRouteUploaded(record.id, objectID: objectID)
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

    /// #256 acceptance (H9): re-sync after a relaunch downloads nothing new.
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

    /// #256 acceptance (H10): a sync interrupted at N of M keeps the N across
    /// a relaunch — the next sync pulls only the remainder.
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
}
