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
