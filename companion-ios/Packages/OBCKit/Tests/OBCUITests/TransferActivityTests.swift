import Testing
import Foundation
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// #459 — the in-flight transfer ledger, and the two writers that feed it:
/// `UploadSheetModel` (claims while an upload is moving) and
/// `RideSyncCoordinator` (claims while a batch is `.syncing`). The ledger is
/// what the background drain waits on, so the claim/release discipline IS the
/// "in-flight transfers are never dropped" guarantee.
@MainActor
struct TransferActivityTests {
    private func eventually(
        _ what: String,
        timeout: Duration = .seconds(30),
        _ condition: @MainActor () -> Bool
    ) async {
        let deadline = ContinuousClock.now.advanced(by: timeout)
        while !condition() {
            if ContinuousClock.now > deadline {
                Issue.record("timed out waiting for \(what)")
                return
            }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    // MARK: The ledger itself

    @Test func idleWaitReturnsImmediately() async {
        let activity = TransferActivity()
        #expect(!activity.isActive)
        await activity.waitUntilIdle()  // must not hang
    }

    @Test func waitResumesWhenTheLastClaimEnds() async {
        let activity = TransferActivity()
        let first = activity.begin()
        let second = activity.begin()

        let resumed = Flag()
        let waiter = Task {
            await activity.waitUntilIdle()
            resumed.value = true
        }

        try? await Task.sleep(for: .milliseconds(50))
        activity.end(first)
        try? await Task.sleep(for: .milliseconds(50))
        #expect(!resumed.value, "one claim still open — the drain keeps waiting")

        activity.end(second)
        await waiter.value
        #expect(resumed.value)
        #expect(!activity.isActive)
    }

    @Test func endIsIdempotentPerToken() async {
        let activity = TransferActivity()
        let claimed = activity.begin()
        let held = activity.begin()
        activity.end(claimed)
        activity.end(claimed)  // a raced double-release must not free `held`'s claim
        #expect(activity.isActive)
        activity.end(held)
        #expect(!activity.isActive)
    }

    @Test func canceledWaiterResumesWithoutTheLedgerDraining() async {
        let activity = TransferActivity()
        let token = activity.begin()
        let waiter = Task { await activity.waitUntilIdle() }
        try? await Task.sleep(for: .milliseconds(50))
        waiter.cancel()
        await waiter.value  // must resume promptly despite the open claim
        #expect(activity.isActive)
        activity.end(token)
    }

    // MARK: The upload sheet's claim

    private func makeUpload(
        _ scenario: Scenario, activity: TransferActivity, payloadBytes: Int = 100_000
    ) -> UploadSheetModel {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        control.throughputBytesPerSec = 40_000_000
        let blob = RouteBlob(
            summary: RouteSummary(
                id: RouteID("ledger-test"), name: "Kettle Moraine Loop",
                distanceMeters: 62_400, elevationGainMeters: 840
            ),
            waypoints: [],
            payload: Data(count: payloadBytes)
        )
        return UploadSheetModel(
            transport: MockTransport(control: control),
            blob: blob,
            deviceName: "Trailhead",
            timing: UploadSheetModel.Timing(doneAutoDismiss: .milliseconds(40)),
            activity: activity
        )
    }

    @Test func uploadClaimsWhileMovingAndReleasesOnDone() async {
        let activity = TransferActivity()
        let model = makeUpload(.happyPath, activity: activity)
        #expect(!activity.isActive)

        model.start()
        #expect(activity.isActive, "the claim opens with the transfer")

        await eventually("F₂") { model.phase == .done }
        #expect(!activity.isActive, "a committed upload releases the claim")
    }

    @Test func interruptedUploadReleasesItsClaim() async {
        let activity = TransferActivity()
        let model = makeUpload(.uploadDrop, activity: activity)
        model.start()
        #expect(activity.isActive)

        // The scenario drops the link mid-transfer: stalled-resumable is NOT
        // in flight — the background drain must not wait on a transfer whose
        // link is already gone.
        await eventually("interrupted") { model.phase == .interrupted }
        #expect(!activity.isActive)
    }

    /// The tick and link-state watchers drain two independent streams, so under
    /// scheduler load a pre-drop progress tick can be *delivered* after the drop
    /// event (this suite's full-parallel flake: backlogged watchers, and the
    /// state subscription replays `.outOfRange`). The parked sheet must
    /// **discard** a stale tick outright — it must not read as "moving again"
    /// (flipping back to `.uploading` and re-claiming the ledger for a transfer
    /// whose link is already gone, permanently, since the parked transfer emits
    /// nothing further), and it must not move the parked bar either.
    @Test func staleTickDeliveredAfterTheDropDoesNotReclaim() async {
        let activity = TransferActivity()
        let transport = HandDrivenUploadTransport()
        let model = UploadSheetModel(
            transport: transport,
            blob: RouteBlob(
                summary: RouteSummary(
                    id: RouteID("stale-tick"), name: "Kettle Moraine Loop",
                    distanceMeters: 62_400, elevationGainMeters: 840
                ),
                waypoints: [],
                payload: Data(count: 100_000)
            ),
            deviceName: "Trailhead",
            timing: UploadSheetModel.Timing(doneAutoDismiss: .milliseconds(40)),
            activity: activity
        )
        model.start()
        #expect(activity.isActive)

        // A live tick moves the bar (and proves the tick watcher is consuming).
        transport.progress.yield(TransferProgress(bytesDone: 10_000, total: 100_000))
        await eventually("first tick") { model.progress.bytesDone == 10_000 }

        // The link drops — the sheet parks and releases its claim.
        transport.states.send(.outOfRange)
        await eventually("interrupted") { model.phase == .interrupted }
        #expect(!activity.isActive)

        // A tick that was in flight before the drop lands late. Sequencing it
        // after `.interrupted` reproduces deterministically what full-suite
        // parallel load produces by starving the MainActor. Discarded ticks
        // leave nothing to wait on, so settle, then assert nothing moved —
        // slow delivery only makes the negative asserts vacuously true.
        transport.progress.yield(TransferProgress(bytesDone: 20_000, total: 100_000))
        try? await Task.sleep(for: .milliseconds(100))
        #expect(model.progress.bytesDone == 10_000, "a stale pre-drop tick must not move the parked bar")
        #expect(model.phase == .interrupted, "…or resurrect .uploading")
        #expect(!activity.isActive, "…or re-claim the ledger")

        // Resume unparks the sheet: ticks apply again (the ordered stream has
        // drained the stale one by the time this lands) and the claim re-opens.
        model.resume()
        #expect(activity.isActive, "resume re-claims the ledger")
        transport.progress.yield(TransferProgress(bytesDone: 30_000, total: 100_000))
        await eventually("post-resume tick") { model.progress.bytesDone == 30_000 }
        #expect(model.phase == .uploading)
    }

    @Test func dismissedSheetReleasesItsClaim() async {
        let activity = TransferActivity()
        let model = makeUpload(.happyPath, activity: activity, payloadBytes: 10_000_000)
        model.start()
        #expect(activity.isActive)

        await eventually("progress movement") { model.progress.bytesDone > 0 }
        model.sheetDismissed()  // cancels the unresolved transfer
        #expect(!activity.isActive, "a torn-down sheet must not hold the grace window open")
    }

    // MARK: The sync coordinator's claim

    @Test func syncClaimsWhileSyncingAndReleasesOnDone() async {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.throughputBytesPerSec = 200_000_000
        let activity = TransferActivity()
        let coordinator = RideSyncCoordinator(
            transport: MockTransport(control: control),
            library: InMemoryLibraryStore(),
            // Sticky holds (the RideSyncCoordinatorTests convention): the
            // ledger must release on the `.done` transition, not a timer race.
            timing: RideSyncCoordinator.Timing(
                syncDoneHold: .seconds(300), syncedLineHold: .seconds(300)),
            activity: activity
        )
        await eventually("link up") { coordinator.connection == .connected }
        #expect(!activity.isActive)

        coordinator.sync()
        await eventually("claim while syncing") { activity.isActive }

        await eventually("batch done") { coordinator.syncState == .done }
        #expect(!activity.isActive, "the `.done` hold is UI pacing, not an in-flight transfer")
    }
}

/// A main-actor flag a free-running waiter task can raise (a captured local
/// `var` can't cross into a `Task` under Swift 6).
@MainActor
private final class Flag {
    var value = false
}

/// A transport whose progress ticks and link states the test delivers by hand,
/// so the tick↔drop ordering is sequenced deterministically — the timing-driven
/// `MockTransport` only produces the stale-tick-after-drop order under
/// scheduler load. Only `state` + `uploadRoute` are exercised; the rest is inert.
private final class HandDrivenUploadTransport: DeviceTransport, @unchecked Sendable {
    let states = AsyncMulticast<ConnectionState>(.connected)
    let progress: AsyncStream<TransferProgress>.Continuation
    private let progressStream: AsyncStream<TransferProgress>
    private let outcomePromise = AsyncPromise<TransferOutcome>()
    private let batteryMulticast = AsyncMulticast<Int>(100)

    init() {
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        progressStream = stream
        progress = continuation
    }

    var state: AsyncStream<ConnectionState> { states.stream() }
    var battery: AsyncStream<Int> { batteryMulticast.stream() }
    var storeChanges: AsyncStream<StoreChanged> { AsyncStream { $0.finish() } }

    func uploadRoute(_ route: RouteBlob) -> TransferHandle {
        TransferHandle(
            progress: progressStream,
            outcome: outcomePromise,
            onCancel: { [outcomePromise] in outcomePromise.fulfill(.canceled) },
            onResume: {}
        )
    }

    // Unreachable in these tests.
    func connect() async throws {}
    func disconnect() async {}
    func deviceInfo() async throws -> DeviceInfo { fatalError("unused") }
    func readConfig() async throws -> DeviceConfig { fatalError("unused") }
    func writeConfig(_ config: DeviceConfig) async throws {}
    func listRoutes() async throws -> [RouteCatalogEntry] { [] }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { fatalError("unused") }
    func deleteRoute(_ id: DeviceObjectID) async throws {}
    func listRides() async throws -> RideCatalog { RideCatalog(rides: []) }
    func rideDetail(_ id: RideID) async throws -> RideDetail { fatalError("unused") }
    func downloadRides(_ ids: [RideID]) -> RideDownload { fatalError("unused") }
    func readDiagnostics() async throws -> Data { Data() }
}
