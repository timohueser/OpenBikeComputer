import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// B7 acceptance, host-side: the ride-sync state machine driven through
/// `MockTransport` — extracted from `MainScreenModelTests` with the coordinator
/// (#358). The SYNC button contract (idle → syncing → done → idle), H9
/// up-to-date, the H10 drop / Resume / fresh-sync-supersession trio, the
/// persistence-per-landed-ride rule, and the hard-failure path. List/reconcile
/// behavior (what the model does with landed rides) stays in
/// `MainScreenModelTests`.
@MainActor
final class RideSyncCoordinatorTests: XCTestCase {
    /// Sticky holds: the done-hold and line-hold timers race the poll loop on
    /// wall-clock time, so a CI scheduling stall can expire `.done` (or the
    /// confirm line) *between* two polls and the wait times out on a state
    /// that's already gone. Holds this long are terminal within a test —
    /// completion waits observe them race-free. The expiry behavior itself is
    /// asserted separately with short timers, waiting only on the terminal
    /// state *after* the timer (stable once reached).
    private static let stickyTiming = RideSyncCoordinator.Timing(
        syncDoneHold: .seconds(300),
        syncedLineHold: .seconds(300)
    )

    private func makeCoordinator(
        _ scenario: Scenario,
        library: any LibraryStore = InMemoryLibraryStore(),
        timing: RideSyncCoordinator.Timing = stickyTiming
    ) -> (RideSyncCoordinator, MockControl) {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        // Fast transfers: the pacing under test is the coordinator's, not the mock's.
        control.throughputBytesPerSec = 200_000_000
        let coordinator = RideSyncCoordinator(
            transport: MockTransport(control: control), library: library, timing: timing)
        return (coordinator, control)
    }

    /// Poll until `condition` holds (the coordinator moves on free-running tasks).
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

    /// The coordinator's link mirror fills from the replayed `state` stream —
    /// wait for it before pressing Sync (the gate reads it synchronously).
    private func startConnected(_ coordinator: RideSyncCoordinator) async {
        await waitFor("link up") { coordinator.connection == .connected }
    }

    // MARK: Stream lifecycle (#356)

    /// The coordinator's own `transport.state` subscription never finishes —
    /// the loop must hold the coordinator weakly so it can deallocate with the
    /// stream still live (the same convention as the model's loops).
    func testStateWatchDoesNotRetainTheCoordinator() async {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        weak var leaked: RideSyncCoordinator?
        do {
            let coordinator = RideSyncCoordinator(
                transport: MockTransport(control: control), library: InMemoryLibraryStore())
            await startConnected(coordinator)
            leaked = coordinator
        }
        // The last strong ref is gone; push an event through the still-open
        // stream so a strongly-capturing loop would show up as a live ref.
        control.connection = .outOfRange
        for _ in 0..<10 { await Task.yield() }
        XCTAssertNil(leaked, "the state watch must hold the coordinator weakly")
    }

    // MARK: The SYNC button contract

    func testFirstSyncPullsEverythingThenIdles() async {
        // Short done-hold so the return to idle is observable; sticky line-hold
        // so asserting the confirm line can't race its own expiry.
        let (coordinator, _) = makeCoordinator(
            .happyPath,
            timing: .init(syncDoneHold: .milliseconds(60), syncedLineHold: .seconds(300)))
        await startConnected(coordinator)

        coordinator.sync()
        // The sticky confirm line is the completion marker; `.done` itself is a
        // 60 ms window here, and reaching `.idle` below proves it ran — the
        // machine only idles out of a completed sync through the done-hold.
        await waitFor("confirm line") { coordinator.lastSyncCount == 4 }
        XCTAssertNil(coordinator.syncProgress)
        await waitFor("done hold expires") { coordinator.syncState == .idle }
        XCTAssertEqual(coordinator.lastSyncCount, 4)   // the line outlives the check
    }

    /// The confirm line expires on its own timer after the check. Both holds
    /// short; the wait targets only the terminal end state (idle button, line
    /// gone), armed *after* durable proof the batch landed (the library) — no
    /// wait ever targets a transient window.
    func testConfirmLineExpiresAfterTheCheck() async {
        let library = InMemoryLibraryStore()
        let (coordinator, _) = makeCoordinator(
            .happyPath, library: library,
            timing: .init(syncDoneHold: .milliseconds(60), syncedLineHold: .milliseconds(60)))
        await startConnected(coordinator)

        coordinator.sync()
        await waitFor("batch lands") { library.rideSummaries().count == 4 }
        // From here the machine walks count-set → done → idle → line-expiry on
        // its own; idle + nil only coexist once the whole sequence has run.
        await waitFor("confirm line expires") {
            coordinator.syncState == .idle && coordinator.lastSyncCount == nil
        }
    }

    func testSecondSyncIsUpToDate() async {
        let (coordinator, _) = makeCoordinator(.happyPath)
        await startConnected(coordinator)

        coordinator.sync()
        await waitFor("first sync done") { coordinator.lastSyncCount == 4 }

        // Re-arm straight from the sticky `.done` — the gate only rejects
        // a *running* sync, so waiting out the done-hold isn't needed.
        coordinator.sync()
        // H9: quiet toast, straight back to idle — never an empty "done".
        await waitFor("up-to-date toast") { coordinator.upToDateToastVisible }
        XCTAssertEqual(coordinator.syncState, .idle)
        XCTAssertNil(coordinator.lastSyncCount)
    }

    func testRideAddedOnDeviceSyncsAsOneNewRide() async {
        let (coordinator, control) = makeCoordinator(.happyPath)
        await startConnected(coordinator)
        var landed: [Ride] = []
        coordinator.onRideLanded = { landed.append($0) }

        coordinator.sync()
        await waitFor("first sync") { coordinator.lastSyncCount == 4 }

        control.emit(.rideAdded(RideSummary(
            id: RideID("ride-new"),
            name: "Lunch Loop",
            date: Date(),
            distanceMeters: 18_000,
            movingTime: 2_800,
            averageSpeedMps: 6.4
        )))

        coordinator.sync()
        await waitFor("one new ride") { coordinator.lastSyncCount == 1 }
        XCTAssertEqual(landed.last?.summary.name, "Lunch Loop")
    }

    /// H10: the drop freezes what landed into the banner state — button idle,
    /// progress down, and the interruption carries the landed counts.
    func testDropMidSyncRaisesH10WithTheLandedCounts() async {
        let (coordinator, control) = makeCoordinator(.happyPath)
        await startConnected(coordinator)

        control.dropTransfer(atFraction: 0.5)
        coordinator.sync()
        await waitFor("H10 raised") { coordinator.syncInterruption != nil }
        XCTAssertEqual(coordinator.syncState, .idle)
        XCTAssertNil(coordinator.syncProgress)
        XCTAssertNil(coordinator.lastSyncCount)

        let interruption = coordinator.syncInterruption
        XCTAssertEqual(interruption?.total, 4)
        XCTAssertGreaterThan(interruption?.landed ?? -1, 0, "half the bytes should land some rides")
        XCTAssertLessThan(interruption?.landed ?? 4, 4, "a drop mid-batch can't have landed them all")

        XCTAssertEqual(coordinator.connection, .outOfRange)
    }

    /// H10 → Resume: the same transfer continues from its last committed
    /// offset and finishes; every ride of the batch counts once.
    func testResumeContinuesTheDroppedSyncToCompletion() async {
        let library = InMemoryLibraryStore()
        let (coordinator, control) = makeCoordinator(.happyPath, library: library)
        await startConnected(coordinator)

        control.dropTransfer(atFraction: 0.5)
        coordinator.sync()
        await waitFor("H10 raised") { coordinator.syncInterruption != nil }
        let landedAtDrop = coordinator.syncInterruption?.landed ?? 0

        coordinator.resumeSync()
        XCTAssertNil(coordinator.syncInterruption, "Resume takes the banner down at once")
        XCTAssertEqual(coordinator.syncState, .syncing)
        XCTAssertEqual(coordinator.syncProgress,
                       .init(done: landedAtDrop, total: 4),
                       "the caption picks up where the drop left it")

        await waitFor("batch completes") {
            coordinator.syncState == .done && coordinator.lastSyncCount == 4
        }
        XCTAssertEqual(coordinator.connection, .connected, "resume restores the link")
        XCTAssertEqual(library.syncedRideIDs().count, 4)
        XCTAssertEqual(library.rideSummaries().count, 4, "resumed rides persist like the rest")
    }

    /// The user can also just sync again once back in range — what landed
    /// stays synced, so the fresh batch is exactly the remainder. This is the
    /// supersession path: the new `sync()` cancels the old task and its
    /// stalled batch before touching shared state.
    func testFreshSyncAfterADropPullsOnlyTheRemainder() async {
        let (coordinator, control) = makeCoordinator(.happyPath)
        await startConnected(coordinator)

        control.dropTransfer(atFraction: 0.5)
        coordinator.sync()
        await waitFor("H10 raised") { coordinator.syncInterruption != nil }

        control.connection = .connected
        await waitFor("reconnect reaches the coordinator") { coordinator.connection == .connected }
        coordinator.sync()
        XCTAssertNil(coordinator.syncInterruption, "a fresh sync clears the waiting banner")
        await waitFor("remainder synced") {
            coordinator.syncState == .done && coordinator.lastSyncCount != nil
        }
        let remainder = coordinator.lastSyncCount ?? 0
        XCTAssertGreaterThan(remainder, 0, "the fresh sync should find the un-landed rides")
        XCTAssertLessThan(remainder, 4, "partial rides must not be re-counted")
    }

    /// B7's decode path: a synced ride lands in the library with its tracklog
    /// decoded from the payload — not as an empty-points summary shell.
    func testSyncedRideCarriesTheDecodedTracklog() async {
        let library = InMemoryLibraryStore()
        let (coordinator, _) = makeCoordinator(.happyPath, library: library)
        await startConnected(coordinator)

        coordinator.sync()
        await waitFor("sync done") { coordinator.syncState == .done }

        let stored = library.rideSummaries()
        XCTAssertEqual(stored.count, 4)
        XCTAssertTrue(stored.allSatisfy { !(library.ridePoints($0.id) ?? []).isEmpty },
                      "every fixture payload decodes into a tracklog")

        let kettle = library.ridePoints(RideID("ride-kettle-moraine"))
        XCTAssertEqual(kettle?.count, 9, "the fixture's track survives the wire")
        let start = kettle?.first
        XCTAssertEqual(start?.coordinate.latitude ?? 0, 42.8672, accuracy: 1e-6)
        XCTAssertEqual(start?.coordinate.longitude ?? 0, -88.4471, accuracy: 1e-6)
        XCTAssertEqual(start?.elevationMeters ?? 0, 264, accuracy: 0.5)
        // Timestamps synthesized across the moving time, in ride order.
        let span = kettle.map { $0.last!.timestamp.timeIntervalSince($0.first!.timestamp) }
        XCTAssertEqual(span ?? 0, 10_260, accuracy: 1)
    }

    func testSyncNoOpsWhenUnreachable() async {
        let (coordinator, _) = makeCoordinator(.outOfRange)
        await waitFor("link state lands") { coordinator.connection == .outOfRange }

        coordinator.sync()
        try? await Task.sleep(for: .milliseconds(80))
        XCTAssertEqual(coordinator.syncState, .idle)
        XCTAssertFalse(coordinator.upToDateToastVisible)
    }

    /// The injected `canSync` veto (#303 lives with the model): a false answer
    /// must not start a transfer — no decode, no toast, no state movement.
    func testCanSyncVetoBlocksTheSync() async {
        let (coordinator, _) = makeCoordinator(.happyPath)
        await startConnected(coordinator)
        coordinator.canSync = { false }

        coordinator.sync()
        try? await Task.sleep(for: .milliseconds(80))
        XCTAssertEqual(coordinator.syncState, .idle)
        XCTAssertNil(coordinator.syncProgress)
        XCTAssertFalse(coordinator.upToDateToastVisible)
    }

    /// A hard transfer failure (`crcMismatch` — the throwing end of the rides
    /// stream, unlike a drop's stall): the batch is over, but `landed` keeps
    /// the partial — what persisted stays persisted, the button returns to
    /// idle with no confirm line and no Resume banner (nothing is resumable).
    func testHardStreamFailureKeepsThePartialAndIdles() async {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        let library = InMemoryLibraryStore()
        let base = MockTransport(control: control)
        let transport = HardFailingDownloadTransport(
            base: base,
            yieldedRides: [
                DownloadedRide(id: RideID("ride-kettle-moraine"), payload: Data()),
                DownloadedRide(id: RideID("ride-sunday-coffee-spin"), payload: Data()),
            ]
        )
        let coordinator = RideSyncCoordinator(
            transport: transport, library: library, timing: Self.stickyTiming)
        await startConnected(coordinator)

        coordinator.sync()
        // The two yielded rides land and persist before the stream throws…
        await waitFor("partial lands") { library.rideSummaries().count == 2 }
        // …then the failed outcome brings the button straight back to idle.
        await waitFor("failure settles") {
            coordinator.syncState == .idle && coordinator.syncProgress == nil
        }
        XCTAssertEqual(library.rideSummaries().count, 2, "the partial batch persists")
        XCTAssertEqual(library.syncedRideIDs().count, 2)
        // A failed outcome is terminal: no confirm line, no H10 banner (the
        // stream finished — there's nothing to resume), no up-to-date toast.
        XCTAssertNil(coordinator.lastSyncCount)
        XCTAssertNil(coordinator.syncInterruption)
        XCTAssertFalse(coordinator.upToDateToastVisible)
    }

    // MARK: Persistence across "relaunches" (B1S)

    /// #256 acceptance (H9): re-sync after a relaunch downloads nothing new.
    func testResyncAfterRelaunchIsUpToDate() async {
        let library = InMemoryLibraryStore()
        let (first, _) = makeCoordinator(.happyPath, library: library)
        await startConnected(first)
        first.sync()
        await waitFor("first sync") { first.lastSyncCount == 4 }
        XCTAssertEqual(library.rideSummaries().count, 4, "each landed ride persists")

        let (relaunched, _) = makeCoordinator(.happyPath, library: library)
        await startConnected(relaunched)
        relaunched.sync()

        await waitFor("H9 across the relaunch") { relaunched.upToDateToastVisible }
        XCTAssertEqual(relaunched.syncState, .idle)
        XCTAssertNil(relaunched.lastSyncCount)
    }

    /// #256 acceptance (H10): a sync interrupted at N of M keeps the N across
    /// a relaunch — the next sync pulls only the remainder.
    func testPartialSyncSurvivesRelaunch() async {
        let library = InMemoryLibraryStore()
        let (first, control) = makeCoordinator(.happyPath, library: library)
        await startConnected(first)
        control.dropTransfer(atFraction: 0.5)
        first.sync()
        await waitFor("drop observed") { first.connection == .outOfRange }
        await waitFor("back to idle") { first.syncState == .idle }

        let landed = library.syncedRideIDs().count
        XCTAssertTrue((1...3).contains(landed), "the drop should leave a partial batch")
        XCTAssertEqual(library.rideSummaries().count, landed, "what landed is already persisted")

        let (relaunched, _) = makeCoordinator(.happyPath, library: library)
        await startConnected(relaunched)
        relaunched.sync()
        await waitFor("remainder synced") { relaunched.syncState == .done }
        XCTAssertEqual(relaunched.lastSyncCount, 4 - landed)
    }

    // MARK: v2 list truncation (spec §7.4)

    /// The bounded ride catalog's truncation signal: a truncated read sets
    /// `hiddenRideCount` (the banner trigger), and a link edge back into
    /// `.connected` clears it **before** any new list read. A count carried
    /// across a reconnect could be stale (the rider freed space while away) or
    /// a different device's entirely (the banner names the connected device) —
    /// unknown-until-read is the honest state.
    func testTruncatedListSetsTheCountAndReconnectClearsIt() async {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.throughputBytesPerSec = 200_000_000
        let coordinator = RideSyncCoordinator(
            transport: TruncatedRideCatalogTransport(
                base: MockTransport(control: control), hiddenRideCount: 3),
            library: InMemoryLibraryStore(), timing: Self.stickyTiming)
        await startConnected(coordinator)

        coordinator.sync()
        await waitFor("truncation count from the list read") { coordinator.hiddenRideCount == 3 }
        // Let the batch land before dropping the link, so the drop below is a
        // clean idle-time edge (not an H10 interruption — separate machinery).
        await waitFor("batch done") { coordinator.syncState == .done }

        control.connection = .disconnected
        await waitFor("link down") { coordinator.connection == .disconnected }
        XCTAssertEqual(coordinator.hiddenRideCount, 3, "the count survives the drop itself")

        control.connection = .connected
        await waitFor("count cleared on the reconnect edge") { coordinator.hiddenRideCount == 0 }
    }
}

/// Forwards everything to the mock, but reports the device's ride catalog as
/// **truncated** (`hiddenRideCount` rides beyond what the list carried) — the
/// v2 header's `total > count` signal the mock's fixture catalog never trips.
private struct TruncatedRideCatalogTransport: DeviceLink, DeviceObjects {
    let base: MockTransport
    let hiddenRideCount: Int

    var state: AsyncStream<ConnectionState> { base.state }
    func connect() async throws { try await base.connect() }
    func disconnect() async { await base.disconnect() }
    func deviceInfo() async throws -> DeviceInfo { try await base.deviceInfo() }
    func listRoutes() async throws -> [RouteCatalogEntry] { try await base.listRoutes() }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { try await base.routeDetail(id) }
    func uploadRoute(_ route: RouteBlob) -> TransferHandle { base.uploadRoute(route) }
    func deleteRoute(_ id: DeviceObjectID) async throws { try await base.deleteRoute(id) }
    func rideDetail(_ id: RideID) async throws -> RideDetail { try await base.rideDetail(id) }
    func downloadRides(_ ids: [RideID]) -> RideDownload { base.downloadRides(ids) }

    func listRides() async throws -> RideCatalog {
        var catalog = try await base.listRides()
        catalog.hiddenRideCount = hiddenRideCount
        return catalog
    }
}

/// Forwards everything to the mock, but hands back a download whose rides
/// stream **throws** after a couple of rides — the hard `crcMismatch` failure
/// the mock's drop knob (a stall, resumable by design) can't stage.
private struct HardFailingDownloadTransport: DeviceLink, DeviceObjects {
    let base: MockTransport
    let yieldedRides: [DownloadedRide]

    var state: AsyncStream<ConnectionState> { base.state }
    func connect() async throws { try await base.connect() }
    func disconnect() async { await base.disconnect() }
    func deviceInfo() async throws -> DeviceInfo { try await base.deviceInfo() }
    func listRoutes() async throws -> [RouteCatalogEntry] { try await base.listRoutes() }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { try await base.routeDetail(id) }
    func uploadRoute(_ route: RouteBlob) -> TransferHandle { base.uploadRoute(route) }
    func deleteRoute(_ id: DeviceObjectID) async throws { try await base.deleteRoute(id) }
    func listRides() async throws -> RideCatalog { try await base.listRides() }
    func rideDetail(_ id: RideID) async throws -> RideDetail { try await base.rideDetail(id) }

    func downloadRides(_ ids: [RideID]) -> RideDownload {
        let (stream, continuation) = AsyncThrowingStream<DownloadedRide, Error>.makeStream()
        for ride in yieldedRides { continuation.yield(ride) }
        continuation.finish(throwing: DeviceError.crcMismatch)
        return RideDownload(handle: .immediatelyFinished(.failed(.crcMismatch)), rides: stream)
    }
}
