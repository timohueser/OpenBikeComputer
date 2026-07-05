import Foundation
import Testing
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The connect-time possession ack (spec §4.4 `ackRides`): on every edge into
/// `.connected` the coordinator sends the library's synced ride ids, so the
/// device's per-ride "synced" flag heals from the phone's ground truth — the
/// fix for rides synced before the device tracked the flag (they were never
/// re-downloaded, so the download-completion event could never reach them).
@MainActor @Suite struct RideAckReconcileTests {
    private func makeCoordinator(
        library: any LibraryStore = InMemoryLibraryStore()
    ) -> (RideSyncCoordinator, MockControl) {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        let coordinator = RideSyncCoordinator(
            transport: MockTransport(control: control), library: library)
        return (coordinator, control)
    }

    /// Poll until `condition` holds (the coordinator moves on free-running
    /// tasks) — the `RideSyncCoordinatorTests` convention, Swift Testing flavor.
    private func waitFor(
        _ what: String,
        timeout: Duration = .seconds(30),
        _ condition: () -> Bool
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

    /// The launch connection acks everything the library holds — the exact
    /// scenario behind the fix: ten pre-sidecar rides show "not synced" on the
    /// device while the app reports "No new rides"; one connect after the
    /// update, the possession ack flips them.
    @Test func connectAcksTheLibrarysSyncedRides() async {
        let library = InMemoryLibraryStore()
        let ids = (1...10).map { RideID(deviceObjectID: DeviceObjectID(UInt16($0))) }
        for id in ids { library.markRideSynced(id) }

        let (coordinator, control) = makeCoordinator(library: library)
        _ = coordinator // kept alive; the ack rides its connection watch
        await waitFor("the connect-time ack") { !control.ackedRideBatches.isEmpty }
        #expect(Set(control.ackedRideBatches.flatMap { $0 }) == Set(ids))
    }

    /// An empty library acks nothing — no zero-length command chatter.
    @Test func emptyLibraryAcksNothing() async {
        let (coordinator, control) = makeCoordinator()
        await waitFor("link up") { coordinator.connection == .connected }
        // Give a wrong-headed ack a beat to land before asserting silence.
        try? await Task.sleep(for: .milliseconds(100))
        #expect(control.ackedRideBatches.isEmpty)
    }

    /// Every reconnect re-acks (that is what makes the ack self-healing: a send
    /// the link dropped is simply covered by the next connect).
    @Test func reconnectAcksAgain() async {
        let library = InMemoryLibraryStore()
        let id = RideID(deviceObjectID: DeviceObjectID(7))
        library.markRideSynced(id)

        let (coordinator, control) = makeCoordinator(library: library)
        await waitFor("the first ack") { control.ackedRideBatches.count == 1 }

        control.connection = .disconnected
        await waitFor("the drop") { coordinator.connection == .disconnected }
        control.connection = .connected
        await waitFor("the reconnect ack") { control.ackedRideBatches.count == 2 }
        #expect(control.ackedRideBatches.allSatisfy { $0 == [id] })
    }
}
