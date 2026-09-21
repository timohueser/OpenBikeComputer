import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The ride-sync state machine driven through `MockTransport`: the sync button contract, the
/// up-to-date case, the drop, Resume and fresh-sync-supersession trio, the persistence-per-landed
/// ride rule, and the hard-failure path. What the model does with landed rides stays in
/// `MainScreenModelTests`.
@MainActor
final class RideSyncCoordinatorTests: XCTestCase {
    /// Sticky holds: the done-hold and line-hold timers race the poll loop on wall-clock time, so
    /// a scheduling stall can expire a state between two polls and the wait then times out on a
    /// state that is already gone. Holds this long are terminal within a test. The expiry
    /// behaviour itself is asserted separately with short timers.
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

    /// The coordinator's link mirror fills from the replayed `state` stream, so wait for it before
    /// pressing Sync: the gate reads it synchronously.
    private func startConnected(_ coordinator: RideSyncCoordinator) async throws {
        try await waitFor("link up") { coordinator.connection == .connected && coordinator.syncState != .syncing }
    }

    // MARK: Stream lifecycle

    /// The coordinator's own state subscription never finishes, so the loop must hold the
    /// coordinator weakly and it can deallocate with the stream still live.
    func testStateWatchDoesNotRetainTheCoordinator() async throws {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        weak var leaked: RideSyncCoordinator?
        do {
            let coordinator = RideSyncCoordinator(
                transport: MockTransport(control: control), library: InMemoryLibraryStore())
            try await startConnected(coordinator)
            leaked = coordinator
        }
        // The last strong ref is gone; push an event through the still-open stream, so a
        // strongly-capturing loop would show up as a live ref.
        control.connection = .outOfRange
        for _ in 0..<10 { await Task.yield() }
        XCTAssertNil(leaked, "the state watch must hold the coordinator weakly")
    }

    // MARK: The SYNC button contract

    func testFirstSyncPullsEverythingThenIdles() async throws {
        // Short done-hold so the return to idle is observable; sticky line-hold so asserting the
        // confirm line cannot race its own expiry.
        let (coordinator, _) = makeCoordinator(
            .happyPath,
            timing: .init(syncDoneHold: .milliseconds(60), syncedLineHold: .seconds(300)))
        try await startConnected(coordinator)

        coordinator.sync()
        // The sticky confirm line is the completion marker. `.done` itself is a 60 ms window here,
        // and reaching `.idle` below proves it ran: the machine only idles out of a completed sync
        // through the done-hold.
        try await waitFor("confirm line") { coordinator.lastSyncCount == 4 }
        XCTAssertNil(coordinator.syncProgress)
        try await waitFor("done hold expires") { coordinator.syncState == .idle }
        XCTAssertEqual(coordinator.lastSyncCount, 4)   // the line outlives the check
    }

    /// The confirm line expires on its own timer after the check. Both holds are short, and the
    /// wait targets only the terminal end state, armed after durable proof the batch landed. No
    /// wait ever targets a transient window.
    func testConfirmLineExpiresAfterTheCheck() async throws {
        let library = InMemoryLibraryStore()
        let (coordinator, _) = makeCoordinator(
            .happyPath, library: library,
            timing: .init(syncDoneHold: .milliseconds(60), syncedLineHold: .milliseconds(60)))
        try await startConnected(coordinator)

        coordinator.sync()
        try await waitFor("batch lands") { library.rideSummaries().count == 4 }
        // From here the machine walks through its states on its own; idle and a cleared line only
        // coexist once the whole sequence has run.
        try await waitFor("confirm line expires") {
            coordinator.syncState == .idle && coordinator.lastSyncCount == nil
        }
    }

    func testSecondSyncIsUpToDate() async throws {
        let (coordinator, _) = makeCoordinator(.happyPath)
        try await startConnected(coordinator)

        coordinator.sync()
        try await waitFor("first sync done") { coordinator.lastSyncCount == 4 }

        // Re-arm straight from the sticky `.done`: the gate only rejects a running sync, so
        // waiting out the done-hold is not needed.
        coordinator.sync()
        // A quiet toast and straight back to idle, never an empty "done".
        try await waitFor("up-to-date toast") { coordinator.upToDateToastVisible }
        XCTAssertEqual(coordinator.syncState, .idle)
        XCTAssertNil(coordinator.lastSyncCount)
    }

    func testRideAddedOnDeviceSyncsAsOneNewRide() async throws {
        let (coordinator, control) = makeCoordinator(.happyPath)
        try await startConnected(coordinator)
        var landed: [Ride] = []
        coordinator.onRideLanded = { landed.append($0) }

        coordinator.sync()
        try await waitFor("first sync") { coordinator.lastSyncCount == 4 }

        control.emit(.rideAdded(RideSummary(
            id: RideID("ride-new"),
            name: "Lunch Loop",
            date: Date(),
            distanceMeters: 18_000,
            movingTime: 2_800,
            averageSpeedMps: 6.4
        )))

        coordinator.sync()
        try await waitFor("one new ride") { coordinator.lastSyncCount == 1 }
        XCTAssertEqual(landed.last?.summary.name, "Lunch Loop")
    }

    /// The drop freezes what landed into the banner state: button idle, progress down, and the
    /// interruption carrying the landed counts.
    func testDropMidSyncRaisesH10WithTheLandedCounts() async throws {
        let (coordinator, control) = makeCoordinator(.happyPath)
        try await startConnected(coordinator)

        control.dropTransfer(atFraction: 0.5)
        coordinator.sync()
        try await waitFor("H10 raised") { coordinator.syncInterruption != nil }
        XCTAssertEqual(coordinator.syncState, .idle)
        XCTAssertNil(coordinator.syncProgress)
        XCTAssertNil(coordinator.lastSyncCount)

        let interruption = coordinator.syncInterruption
        XCTAssertEqual(interruption?.total, 4)
        XCTAssertGreaterThan(interruption?.landed ?? -1, 0, "half the bytes should land some rides")
        XCTAssertLessThan(interruption?.landed ?? 4, 4, "a drop mid-batch can't have landed them all")

        XCTAssertEqual(coordinator.connection, .outOfRange)
    }

    /// Resume continues the same transfer from its last committed offset and finishes; every ride
    /// of the batch counts once.
    func testResumeContinuesTheDroppedSyncToCompletion() async throws {
        let library = InMemoryLibraryStore()
        let (coordinator, control) = makeCoordinator(.happyPath, library: library)
        try await startConnected(coordinator)

        control.dropTransfer(atFraction: 0.5)
        coordinator.sync()
        try await waitFor("H10 raised") { coordinator.syncInterruption != nil }
        let landedAtDrop = coordinator.syncInterruption?.landed ?? 0

        coordinator.resumeSync()
        XCTAssertNil(coordinator.syncInterruption, "Resume takes the banner down at once")
        XCTAssertEqual(coordinator.syncState, .syncing)
        XCTAssertEqual(coordinator.syncProgress,
                       .init(done: landedAtDrop, total: 4),
                       "the caption picks up where the drop left it")

        try await waitFor("batch completes") {
            coordinator.syncState == .done && coordinator.lastSyncCount == 4
        }
        XCTAssertEqual(coordinator.connection, .connected, "resume restores the link")
        XCTAssertEqual(library.syncedRideIDs().count, 4)
        XCTAssertEqual(library.rideSummaries().count, 4, "resumed rides persist like the rest")
    }

    /// The rider can also just sync again once back in range: what landed stays synced, so the
    /// fresh batch is exactly the remainder. This is the supersession path, where the new `sync()`
    /// cancels the old task and its stalled batch before touching shared state.
    func testFreshSyncAfterADropPullsOnlyTheRemainder() async throws {
        let (coordinator, control) = makeCoordinator(.happyPath)
        try await startConnected(coordinator)

        control.dropTransfer(atFraction: 0.5)
        coordinator.sync()
        try await waitFor("H10 raised") { coordinator.syncInterruption != nil }

        control.connection = .connected
        try await waitFor("reconnect reaches the coordinator") { coordinator.connection == .connected }
        coordinator.sync()
        XCTAssertNil(coordinator.syncInterruption, "a fresh sync clears the waiting banner")
        try await waitFor("remainder synced") {
            coordinator.syncState == .done && coordinator.lastSyncCount != nil
        }
        let remainder = coordinator.lastSyncCount ?? 0
        XCTAssertGreaterThan(remainder, 0, "the fresh sync should find the un-landed rides")
        XCTAssertLessThan(remainder, 4, "partial rides must not be re-counted")
    }

    /// A synced ride lands in the library with its tracklog decoded from the payload, not as an
    /// empty-points summary shell.
    func testSyncedRideCarriesTheDecodedTracklog() async throws {
        let library = InMemoryLibraryStore()
        let (coordinator, _) = makeCoordinator(.happyPath, library: library)
        try await startConnected(coordinator)

        coordinator.sync()
        try await waitFor("sync done") { coordinator.syncState == .done }

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

    func testSyncNoOpsWhenUnreachable() async throws {
        let (coordinator, _) = makeCoordinator(.outOfRange)
        try await waitFor("link state lands") { coordinator.connection == .outOfRange }

        coordinator.sync()
        let stayedIdle = await neverHolds({
            coordinator.syncState != .idle || coordinator.upToDateToastVisible
        }, for: .milliseconds(80))
        XCTAssertTrue(stayedIdle, "an unreachable link must not start sync or show a success toast")
        XCTAssertEqual(coordinator.syncState, .idle)
        XCTAssertFalse(coordinator.upToDateToastVisible)
    }

    /// The injected `canSync` veto: a false answer must not start a transfer. No decode, no toast,
    /// no state movement.
    func testCanSyncVetoBlocksTheSync() async throws {
        let (coordinator, _) = makeCoordinator(.happyPath)
        try await startConnected(coordinator)
        coordinator.canSync = { false }

        coordinator.sync()
        let stayedBlocked = await neverHolds({
            coordinator.syncProgress != nil || coordinator.upToDateToastVisible
                || coordinator.syncState == .done
        }, for: .milliseconds(80))
        XCTAssertTrue(stayedBlocked, "a sync veto must prevent progress and success")
        XCTAssertEqual(coordinator.syncState, .idle)
        XCTAssertNil(coordinator.syncProgress)
        XCTAssertFalse(coordinator.upToDateToastVisible)
    }

    /// A hard transfer failure is the throwing end of the rides stream, unlike a drop's stall. The
    /// batch is over, but what persisted stays persisted, and the button returns to idle with no
    /// confirm line and no Resume banner, because nothing is resumable.
    func testHardStreamFailureKeepsThePartialAndIdles() async throws {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        let library = InMemoryLibraryStore()
        let base = MockTransport(control: control)
        let transport = ScriptedDownloadTransport(
            base: base,
            yieldedRides: control.fixtures.rides.prefix(2).map {
                DownloadedRide(id: $0.summary.id, payload: RideObjectCodec.encode($0.ride()))
            }
        )
        let coordinator = RideSyncCoordinator(
            transport: transport, library: library, timing: Self.stickyTiming)
        try await startConnected(coordinator)

        coordinator.sync()
        // The two yielded rides land and persist before the stream throws.
        try await waitFor("partial lands") { library.rideSummaries().count == 2 }
        // Then the failed outcome brings the button straight back to idle.
        try await waitFor("failure settles") {
            coordinator.syncState == .idle && coordinator.syncProgress == nil
        }
        XCTAssertEqual(library.rideSummaries().count, 2, "the partial batch persists")
        XCTAssertEqual(library.syncedRideIDs().count, 2)
        // A retry starts a fresh batch; the two saved rides remain excluded.
        XCTAssertNil(coordinator.lastSyncCount)
        XCTAssertEqual(coordinator.syncInterruption?.landed, 2)
        XCTAssertFalse(coordinator.upToDateToastVisible)
    }

    func testLocalFailureKeepsEarlierSavesAndRetriesAfterRelaunch() async throws {
        for malformedPayload in [false, true] {
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: directory) }
            let library = FileLibraryStore(directory: directory)
            let control = MockControl(scenario: .happyPath)
            control.latency = .zero
            let entries = Array(control.fixtures.rides.prefix(2))
            let first = entries[0]
            let second = entries[1]
            let blocker = directory.appendingPathComponent("rides/\(second.summary.id.rawValue)/points.json")
            if !malformedPayload {
                try FileManager.default.createDirectory(at: blocker, withIntermediateDirectories: true)
            }
            let transport = ScriptedDownloadTransport(
                base: MockTransport(control: control),
                yieldedRides: [
                    DownloadedRide(id: first.summary.id, payload: RideObjectCodec.encode(first.ride())),
                    DownloadedRide(id: second.summary.id,
                                   payload: malformedPayload ? Data() : RideObjectCodec.encode(second.ride())),
                ],
                failure: nil
            )
            let coordinator = RideSyncCoordinator(
                transport: transport, library: library, timing: Self.stickyTiming)
            var landed: [RideID] = []
            coordinator.onRideLanded = { landed.append($0.id) }
            try await startConnected(coordinator)
            coordinator.sync()
            try await waitFor("first ride saved") { landed == [first.summary.id] }
            try await waitFor("local failure settles") {
                coordinator.syncState == .idle && coordinator.syncProgress == nil
            }

            XCTAssertEqual(landed, [first.summary.id])
            XCTAssertEqual(library.syncedRideIDs(), [first.summary.id])
            XCTAssertEqual(library.rideSummaries().map(\.id), [first.summary.id])
            XCTAssertFalse(try XCTUnwrap(library.ridePoints(first.summary.id)).isEmpty)
            XCTAssertNil(coordinator.lastSyncCount, "transport completion cannot override a local failure")
            XCTAssertEqual(coordinator.syncInterruption?.landed, 1)
            XCTAssertFalse(coordinator.upToDateToastVisible)

            if !malformedPayload { try FileManager.default.removeItem(at: blocker) }
            let (relaunched, _) = makeCoordinator(.happyPath, library: FileLibraryStore(directory: directory))
            try await startConnected(relaunched)
            relaunched.sync()
            try await waitFor("unfinished rides saved") { relaunched.syncState == .done }
            XCTAssertEqual(relaunched.lastSyncCount, control.fixtures.rides.count - 1)
            XCTAssertEqual(library.syncedRideIDs().count, control.fixtures.rides.count)
            XCTAssertTrue(library.rideSummaries().allSatisfy { !(library.ridePoints($0.id) ?? []).isEmpty })
        }
    }

    func testExactSourceReplacesOldRevisionAndNeverUsesHistoryAsProof() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let library = FileLibraryStore(directory: directory)
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        let fixture = try XCTUnwrap(control.fixtures.rides.first).ride()
        let scope = LibraryScope(serial: "archive-test", storeID: String(repeating: "a", count: 32))
        let id = RideID(deviceObjectID: DeviceObjectID(42), scope: scope)
        var original = fixture
        original.summary = RideSummary(id: id, name: fixture.summary.name, date: fixture.summary.date,
                                       distanceMeters: fixture.summary.distanceMeters)
        let payload = RideObjectCodec.encode(original)
        for revision: UInt64 in [1, 2] {
            let source = RideSource(storeID: scope.storeID, objectID: 42, revision: revision,
                                    payloadLength: UInt64(payload.count), payloadCRC32: CRC32.checksum(payload))
            var catalogSummary = original.summary
            catalogSummary.name = "Stale catalog display name"
            catalogSummary.source = source
            library.markRideSynced(id)
            let transport = ScriptedDownloadTransport(
                base: MockTransport(control: control),
                yieldedRides: [DownloadedRide(id: id, payload: payload, source: source)], failure: nil,
                catalog: RideCatalog(rides: [catalogSummary]))
            let coordinator = RideSyncCoordinator(transport: transport, library: library, timing: Self.stickyTiming)
            try await startConnected(coordinator)
            coordinator.sync()
            try await waitFor("exact revision archived") { coordinator.syncState == .done }
            XCTAssertEqual(library.archivedRideSource(id), source)
            XCTAssertEqual(library.rideSummaries().first?.name, original.summary.name)
            let next = RideSyncCoordinator(transport: transport, library: library, timing: Self.stickyTiming)
            try await startConnected(next)
            next.sync()
            try await waitFor("committed source is current") { next.upToDateToastVisible }
        }
    }

    func testCatalogAndDownloadedSourceMismatchCannotArchive() async throws {
        let library = InMemoryLibraryStore()
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        let fixture = try XCTUnwrap(control.fixtures.rides.first).ride()
        let scope = LibraryScope(serial: "archive-test", storeID: String(repeating: "a", count: 32))
        let id = RideID(deviceObjectID: DeviceObjectID(42), scope: scope)
        let payload = RideObjectCodec.encode(fixture)
        let source = RideSource(storeID: scope.storeID, objectID: 42, revision: 1,
                                payloadLength: UInt64(payload.count), payloadCRC32: CRC32.checksum(payload))
        var summary = RideSummary(id: id, name: "Ride", date: fixture.summary.date, distanceMeters: 1)
        summary.source = source
        let wrong = RideSource(storeID: scope.storeID, objectID: 42, revision: 2,
                               payloadLength: source.payloadLength, payloadCRC32: source.payloadCRC32)
        let transport = ScriptedDownloadTransport(
            base: MockTransport(control: control),
            yieldedRides: [DownloadedRide(id: id, payload: payload, source: wrong)], failure: nil,
            catalog: RideCatalog(rides: [summary]))
        let coordinator = RideSyncCoordinator(transport: transport, library: library, timing: Self.stickyTiming)
        try await startConnected(coordinator)
        coordinator.sync()
        try await waitFor("mismatched source rejected") {
            coordinator.syncState == .idle && coordinator.syncProgress == nil
        }
        XCTAssertTrue(library.rideSummaries().isEmpty)
        XCTAssertTrue(library.syncedRideIDs().isEmpty)
        XCTAssertNil(coordinator.lastSyncCount)
    }

    // MARK: Persistence across "relaunches"

    /// Re-sync after a relaunch downloads nothing new.
    func testResyncAfterRelaunchIsUpToDate() async throws {
        let library = InMemoryLibraryStore()
        let (first, _) = makeCoordinator(.happyPath, library: library)
        try await startConnected(first)
        first.sync()
        try await waitFor("first sync") { first.lastSyncCount == 4 }
        XCTAssertEqual(library.rideSummaries().count, 4, "each landed ride persists")

        let (relaunched, _) = makeCoordinator(.happyPath, library: library)
        try await startConnected(relaunched)
        relaunched.sync()

        try await waitFor("H9 across the relaunch") { relaunched.upToDateToastVisible }
        XCTAssertEqual(relaunched.syncState, .idle)
        XCTAssertNil(relaunched.lastSyncCount)
    }

    /// A sync interrupted partway keeps what landed across a relaunch, so the next sync pulls only
    /// the remainder.
    func testPartialSyncSurvivesRelaunch() async throws {
        let library = InMemoryLibraryStore()
        let (first, control) = makeCoordinator(.happyPath, library: library)
        try await startConnected(first)
        control.dropTransfer(atFraction: 0.5)
        first.sync()
        try await waitFor("drop observed") { first.connection == .outOfRange }
        try await waitFor("back to idle") { first.syncState == .idle }

        let landed = library.syncedRideIDs().count
        XCTAssertTrue((1...3).contains(landed), "the drop should leave a partial batch")
        XCTAssertEqual(library.rideSummaries().count, landed, "what landed is already persisted")

        let (relaunched, _) = makeCoordinator(.happyPath, library: library)
        try await startConnected(relaunched)
        relaunched.sync()
        try await waitFor("remainder synced") { relaunched.syncState == .done }
        XCTAssertEqual(relaunched.lastSyncCount, 4 - landed)
    }

    // MARK: List truncation

    /// The bounded ride catalog's truncation signal: a truncated read sets the hidden count, which
    /// is the banner trigger, and a link edge back into `.connected` clears it before any new list
    /// read. A count carried across a reconnect could be stale, or a different device's entirely,
    /// and unknown-until-read is the honest state.
    func testTruncatedListSetsTheCountAndReconnectClearsIt() async throws {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.throughputBytesPerSec = 200_000_000
        let coordinator = RideSyncCoordinator(
            transport: TruncatedRideCatalogTransport(
                base: MockTransport(control: control), hiddenRideCount: 3),
            library: InMemoryLibraryStore(), timing: Self.stickyTiming)
        try await startConnected(coordinator)

        coordinator.sync()
        try await waitFor("truncation count from the list read") { coordinator.hiddenRideCount == 3 }
        // Let the batch land before dropping the link, so the drop below is a clean idle-time
        // edge and not an interruption, which is separate machinery.
        try await waitFor("batch done") { coordinator.syncState == .done }

        control.connection = .disconnected
        try await waitFor("link down") { coordinator.connection == .disconnected }
        XCTAssertEqual(coordinator.hiddenRideCount, 3, "the count survives the drop itself")

        control.connection = .connected
        try await waitFor("count cleared on the reconnect edge") { coordinator.hiddenRideCount == 0 }
    }
}

/// Forwards everything to the mock, but reports the device's ride catalog as truncated: the
/// signal the mock's fixture catalog never trips.
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

/// A finite batch with an already-completed handle, or a terminal stream failure.
private struct ScriptedDownloadTransport: DeviceLink, DeviceObjects {
    let base: MockTransport
    let yieldedRides: [DownloadedRide]
    var failure: DeviceError? = .crcMismatch
    var catalog: RideCatalog? = nil

    var state: AsyncStream<ConnectionState> { base.state }
    func connect() async throws { try await base.connect() }
    func disconnect() async { await base.disconnect() }
    func deviceInfo() async throws -> DeviceInfo { try await base.deviceInfo() }
    func listRoutes() async throws -> [RouteCatalogEntry] { try await base.listRoutes() }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { try await base.routeDetail(id) }
    func uploadRoute(_ route: RouteBlob) -> TransferHandle { base.uploadRoute(route) }
    func deleteRoute(_ id: DeviceObjectID) async throws { try await base.deleteRoute(id) }
    func confirmRideArchive(_ receipt: RideArchiveReceipt) async throws -> RideArchiveConfirmation { .confirmed }
    func listRides() async throws -> RideCatalog {
        if let catalog { return catalog }
        return try await base.listRides()
    }
    func rideDetail(_ id: RideID) async throws -> RideDetail { try await base.rideDetail(id) }

    func downloadRides(_ ids: [RideID]) -> RideDownload {
        let (stream, continuation) = AsyncThrowingStream<DownloadedRide, Error>.makeStream()
        for ride in yieldedRides { continuation.yield(ride) }
        continuation.finish(throwing: failure)
        return RideDownload(handle: .immediatelyFinished(failure.map { .failed($0) } ?? .completed), rides: stream)
    }
}
