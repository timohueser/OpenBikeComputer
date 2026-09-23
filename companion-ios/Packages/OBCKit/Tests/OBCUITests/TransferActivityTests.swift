import Testing
import Foundation
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The in-flight transfer ledger and its two writers: `UploadSheetModel` claims while an upload
/// moves, `RideSyncCoordinator` claims while a batch syncs. The background drain waits on the
/// ledger, so the claim and release discipline is the "no dropped transfer" guarantee.
@MainActor
struct TransferActivityTests {

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
        activity.end(first)
        let waiter = Task {
            await activity.waitUntilIdle()
            resumed.value = true
        }

        let stayedParked = await neverHolds({ resumed.value }, for: .milliseconds(50))
        #expect(stayedParked, "one claim still open — the drain keeps waiting")
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

    @Test func uploadClaimsWhileMovingAndReleasesOnDone() async throws {
        let activity = TransferActivity()
        let model = makeUpload(.happyPath, activity: activity)
        #expect(!activity.isActive)

        model.start()
        #expect(activity.isActive, "the claim opens with the transfer")

        try await waitFor("F₂") { model.phase == .done }
        #expect(!activity.isActive, "a committed upload releases the claim")
    }

    @Test func interruptedUploadReleasesItsClaim() async throws {
        let activity = TransferActivity()
        let model = makeUpload(.uploadDrop, activity: activity)
        model.start()
        #expect(activity.isActive)

        // Stalled-resumable is not in flight: the drain must not wait on a transfer whose link
        // is already gone.
        try await waitFor("interrupted") { model.phase == .interrupted }
        #expect(!activity.isActive)
    }

    /// The tick and link-state watchers drain two independent streams, so a pre-drop progress
    /// tick can arrive after the drop event. The parked sheet must discard a stale tick: it must
    /// not move the parked bar, flip back to `.uploading`, or re-claim the ledger.
    @Test func staleTickDeliveredAfterTheDropDoesNotReclaim() async throws {
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

        // A live tick moves the bar and proves the tick watcher is consuming.
        transport.progress.yield(TransferProgress(bytesDone: 10_000, total: 100_000))
        try await waitFor("first tick") { model.progress.bytesDone == 10_000 }

        transport.states.send(.outOfRange)
        try await waitFor("interrupted") { model.phase == .interrupted }
        #expect(!activity.isActive)

        // A tick in flight before the drop lands late. Sequencing it after `.interrupted` makes
        // the race deterministic. A discarded tick leaves nothing to wait on, so settle first,
        // then assert that nothing moved.
        transport.progress.yield(TransferProgress(bytesDone: 20_000, total: 100_000))
        let stayedParked = await neverHolds({
            model.progress.bytesDone != 10_000 || model.phase != .interrupted
                || activity.isActive
        }, for: .milliseconds(100))
        #expect(stayedParked, "a stale tick must not change the parked upload")
        #expect(model.progress.bytesDone == 10_000, "a stale pre-drop tick must not move the parked bar")
        #expect(model.phase == .interrupted, "…or resurrect .uploading")
        #expect(!activity.isActive, "…or re-claim the ledger")

        // Resume unparks the sheet: ticks apply again (the ordered stream has drained the stale
        // one by now) and the claim re-opens.
        model.resume()
        #expect(activity.isActive, "resume re-claims the ledger")
        transport.progress.yield(TransferProgress(bytesDone: 30_000, total: 100_000))
        try await waitFor("post-resume tick") { model.progress.bytesDone == 30_000 }
        #expect(model.phase == .uploading)
    }

    @Test func dismissedSheetReleasesItsClaim() async throws {
        let activity = TransferActivity()
        let model = makeUpload(.happyPath, activity: activity, payloadBytes: 10_000_000)
        model.start()
        #expect(activity.isActive)

        try await waitFor("progress movement") { model.progress.bytesDone > 0 }
        model.sheetDismissed()  // cancels the unresolved transfer
        #expect(!activity.isActive, "a torn-down sheet must not hold the grace window open")
    }

    // MARK: The sync coordinator's claim

    @Test func syncClaimsWhileSyncingAndReleasesOnDone() async throws {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.throughputBytesPerSec = 200_000_000
        let activity = TransferActivity()
        let coordinator = RideSyncCoordinator(
            transport: MockTransport(control: control),
            library: InMemoryLibraryStore(),
            // Sticky holds: the ledger must release on the `.done` transition, not win a timer race.
            timing: RideSyncCoordinator.Timing(
                syncDoneHold: .seconds(300), syncedLineHold: .seconds(300)),
            activity: activity
        )
        try await waitFor("link up") { coordinator.connection == .connected }
        #expect(!activity.isActive)

        coordinator.sync()
        try await waitFor("claim while syncing") { activity.isActive }

        try await waitFor("batch done") { coordinator.syncState == .done }
        #expect(!activity.isActive, "the `.done` hold is UI pacing, not an in-flight transfer")
    }
}

/// A main-actor flag a free-running waiter task can raise; a captured local `var` cannot cross
/// into a `Task` under Swift 6.
@MainActor
private final class Flag {
    var value = false
}

/// A transport whose progress ticks and link states the test delivers by hand, so the tick and
/// drop order is deterministic; `MockTransport` produces that order only under scheduler load.
/// Only `state` and `uploadRoute` are live.
private final class HandDrivenUploadTransport: DeviceLink, DeviceObjects, @unchecked Sendable {
    let states = AsyncMulticast<ConnectionState>(.connected)
    let progress: AsyncStream<TransferProgress>.Continuation
    private let progressStream: AsyncStream<TransferProgress>
    private let outcomePromise = AsyncPromise<TransferOutcome>()

    init() {
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        progressStream = stream
        progress = continuation
    }

    var state: AsyncStream<ConnectionState> { states.stream() }

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
    func listRoutes() async throws -> [RouteCatalogEntry] { [] }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { fatalError("unused") }
    func deleteRoute(_ id: DeviceObjectID) async throws {}
    func listRides() async throws -> RideCatalog { RideCatalog(rides: []) }
    func downloadRides(_ ids: [RideID]) -> RideDownload { fatalError("unused") }
}
