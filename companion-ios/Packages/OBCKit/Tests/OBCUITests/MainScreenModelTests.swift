import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// B3 acceptance, host-side: the main-screen model driven through
/// `MockTransport` — lists from fixtures, the live device cluster, search,
/// the SYNC button state machine, and swipe-delete's data path.
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
        timing: MainScreenModel.Timing = fastTiming
    ) -> (MainScreenModel, MockControl) {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        // Fast transfers: the pacing under test is the model's, not the mock's.
        control.throughputBytesPerSec = 200_000_000
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

    func testDropMidSyncKeepsPartialAndReturnsToIdle() async {
        let (model, control) = makeModel(.happyPath)
        await startLoaded(model)

        control.dropTransfer(atFraction: 0.5)
        model.sync()
        // The drop flips the link out of range; the button quietly returns to
        // idle (the H10 interrupted-banner + resume flow is B7's).
        await waitFor("drop observed") { model.connection == .outOfRange }
        await waitFor("back to idle") { model.syncState == .idle }
        XCTAssertNil(model.lastSyncCount)
        XCTAssertNil(model.syncProgress)

        // What landed stays synced: the next sync pulls only the remainder.
        control.connection = .connected
        await waitFor("reconnect reaches the model") { model.connection == .connected }
        model.sync()
        await waitFor("remainder synced") {
            model.syncState == .done && model.lastSyncCount != nil
        }
        let remainder = model.lastSyncCount ?? 0
        XCTAssertGreaterThan(remainder, 0, "resumed sync should find the un-landed rides")
        XCTAssertLessThan(remainder, 4, "partial rides must not be re-counted")
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

    func testDeleteRouteRemovesLocallyAndOnDevice() async {
        let (model, control) = makeModel(.happyPath)
        await startLoaded(model)

        let id = model.routes[0].id
        model.deleteRoute(id)
        XCTAssertEqual(model.routes.count, 4)   // optimistic removal
        await waitFor("device delete persists") { control.fixtures.routes.count == 4 }

        model.reload()
        await waitFor("reload") { model.loadState == .loaded }
        XCTAssertFalse(model.routes.contains { $0.id == id })
    }

    func testDeleteRideRemovesLocallyAndStaysOutOfNewCounts() async {
        let (model, _) = makeModel(.happyPath)
        await startLoaded(model)

        let id = model.rides[0].id
        model.deleteRide(id)
        XCTAssertEqual(model.rides.count, 3)

        // The deleted ride must not come back as a "new" sync count.
        model.sync()
        await waitFor("sync done") { model.syncState == .done }
        XCTAssertEqual(model.lastSyncCount, 3)
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

        XCTAssertEqual(relaunched.routes.count, 6, "the saved import joins the device's five")
        XCTAssertEqual(relaunched.routes.first?.id, record.id, "phone-only routes stay on top")
        XCTAssertEqual(relaunched.routes.first?.name, "Schwarzwald Day 2")
        let kept = relaunched.importedDetail(for: record.id)
        XCTAssertEqual(kept?.waypoints.count, 1, "the parsed detail is derivable after relaunch")

        relaunched.deleteRoute(record.id)
        XCTAssertTrue(library.plannedRoutes().isEmpty, "delete reaches the library")
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

        let (model, _) = makeModel(.readError, library: library)
        model.start()
        await waitFor("read failure") { model.loadState == .failed }

        XCTAssertEqual(model.routes.map(\.id), [record.id], "planned stays browsable")
        XCTAssertEqual(model.rides.map(\.id), [ride.id], "tracked stays browsable")
    }
}
