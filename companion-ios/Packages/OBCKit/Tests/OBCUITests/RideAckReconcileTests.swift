import Foundation
import Testing
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The connect-time possession ack (spec §4.4 `ackRides`): once per established
/// connection the model has its coordinator send the library's synced ride ids,
/// so the device's per-ride "synced" flag heals from the phone's ground truth —
/// the fix for rides synced before the device tracked the flag (they were never
/// re-downloaded, so the download-completion event could never reach them).
///
/// The ack is triggered by the model's **identity read settling** — never by the
/// raw connect edge — so an id-keyed write can never race the #303
/// protocol-version verdict (the ordering protocol v2's store-epoch check will
/// also rely on).
@MainActor @Suite struct RideAckReconcileTests {
    private func makeModel(
        library: any LibraryStore = InMemoryLibraryStore()
    ) -> (MainScreenModel, MockControl) {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        let model = MainScreenModel(
            transport: MockTransport(control: control), library: library)
        return (model, control)
    }

    /// Poll until `condition` holds (the model moves on free-running tasks) —
    /// the `RideSyncCoordinatorTests` convention, Swift Testing flavor.
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
    @Test func launchAcksTheLibrarysSyncedRides() async {
        let library = InMemoryLibraryStore()
        let ids = (1...10).map { RideID(deviceObjectID: DeviceObjectID(UInt16($0))) }
        for id in ids { library.markRideSynced(id) }

        let (model, control) = makeModel(library: library)
        model.start()
        await waitFor("the connect-time ack") { !control.ackedRideBatches.isEmpty }
        #expect(Set(control.ackedRideBatches.flatMap { $0 }) == Set(ids))
    }

    /// An empty library acks nothing — no zero-length command chatter.
    @Test func emptyLibraryAcksNothing() async {
        let (model, control) = makeModel()
        model.start()
        // The identity read settling is what would have fired the ack.
        await waitFor("the identity read") { model.deviceName == control.deviceInfo.name }
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

        let (model, control) = makeModel(library: library)
        model.start()
        await waitFor("the first ack") { control.ackedRideBatches.count == 1 }

        control.connection = .disconnected
        await waitFor("the drop") { model.connection == .disconnected }
        control.connection = .connected
        await waitFor("the reconnect ack") { control.ackedRideBatches.count == 2 }
        #expect(control.ackedRideBatches.allSatisfy { $0 == [id] })
    }

    /// The #303 ordering regression test: an incompatible device must never
    /// receive the possession ack — the id-keyed write waits for the identity
    /// verdict instead of racing it (before the fix, the connect-edge ack fired
    /// while `deviceInfo()` was still in flight).
    @Test func protocolMismatchBlocksTheAck() async {
        let library = InMemoryLibraryStore()
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(3)))

        let (model, control) = makeModel(library: library)
        control.deviceInfo = DeviceInfo(
            name: control.deviceInfo.name,
            firmwareVersion: control.deviceInfo.firmwareVersion,
            hardwareVersion: control.deviceInfo.hardwareVersion,
            serial: control.deviceInfo.serial,
            protocolVersion: OBCProtocol.version + 1
        )
        model.start()
        await waitFor("the mismatch verdict") { model.protocolMismatch != nil }
        // The verdict landed — any ack racing it would have been sent by now
        // (and the settled gate must keep every later one closed too).
        try? await Task.sleep(for: .milliseconds(100))
        #expect(control.ackedRideBatches.isEmpty)
    }
}
