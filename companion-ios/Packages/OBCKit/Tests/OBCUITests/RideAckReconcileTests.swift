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
/// protocol-version verdict. Protocol v2 (#769) hardens both halves of that
/// ordering: the ack sends only ids scoped to the connected device's
/// **(serial, epoch)** identity, and a read that fails to establish that
/// identity (no epoch, failed read) keeps the ack **closed** for the
/// connection — fail-closed, where #764's v1 posture settled open.
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

    /// The mock device's own (serial, epoch) scope — synced ids must carry it
    /// to be ack-eligible (#769), exactly like ids the transport minted from
    /// this device's catalog.
    private func scope(of control: MockControl) -> LibraryScope {
        control.deviceInfo.libraryScope!
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

    /// The launch connection acks everything the library holds **under the
    /// connected device's scope** — the exact scenario behind the fix: ten
    /// pre-sidecar rides show "not synced" on the device while the app reports
    /// "No new rides"; one connect after the update, the possession ack flips
    /// them.
    @Test func launchAcksTheLibrarysSyncedRides() async {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(library: library)
        let ids = (1...10).map {
            RideID(deviceObjectID: DeviceObjectID(UInt16($0)), scope: scope(of: control))
        }
        for id in ids { library.markRideSynced(id) }

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
        let (model, control) = makeModel(library: library)
        let id = RideID(deviceObjectID: DeviceObjectID(7), scope: scope(of: control))
        library.markRideSynced(id)

        model.start()
        await waitFor("the first ack") { control.ackedRideBatches.count == 1 }

        control.connection = .disconnected
        await waitFor("the drop") { model.connection == .disconnected }
        control.connection = .connected
        await waitFor("the reconnect ack") { control.ackedRideBatches.count == 2 }
        #expect(control.ackedRideBatches.allSatisfy { $0 == [id] })
    }

    /// The #769 scope filter: ids from another device, another era, or the
    /// unclaimed flat legacy namespace never reach `ackRides` — stamping
    /// checkmarks for object ids that happen to collide is the 2026-07-12
    /// incident. (The flat ride ids here are deliberately non-listed ones, so
    /// the claim migration corroborates nothing and they stay flat.)
    @Test func ackSendsOnlyTheConnectedScopesIDs() async {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(library: library)
        let mine = RideID(deviceObjectID: DeviceObjectID(3), scope: scope(of: control))
        let otherSerial = RideID(
            deviceObjectID: DeviceObjectID(3),
            scope: LibraryScope(serial: "OBC-24-999999", epoch: scope(of: control).epoch))
        let otherEra = RideID(
            deviceObjectID: DeviceObjectID(3),
            scope: LibraryScope(serial: scope(of: control).serial, epoch: 0xDEAD_0000))
        let flatLegacy = RideID(deviceObjectID: DeviceObjectID(200))
        for id in [mine, otherSerial, otherEra, flatLegacy] { library.markRideSynced(id) }

        model.start()
        await waitFor("the connect-time ack") { !control.ackedRideBatches.isEmpty }
        #expect(control.ackedRideBatches.flatMap { $0 } == [mine])
    }

    /// Fail-closed (#769): an identity read that carries **no epoch** (a
    /// short/torn v2 read — `storeEpoch == nil`, never a fabricated 0) must
    /// keep `ackRides` closed for the connection, while browsing works.
    @Test func missingEpochBlocksTheAck() async {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(library: library)
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(3), scope: scope(of: control)))
        let current = control.deviceInfo
        control.deviceInfo = DeviceInfo(
            name: current.name,
            firmwareVersion: current.firmwareVersion,
            hardwareVersion: current.hardwareVersion,
            serial: current.serial,
            protocolVersion: current.protocolVersion,
            storeEpoch: nil
        )

        model.start()
        // Identity settles (the SYNC gate stops waiting) — but with no scope.
        await waitFor("the identity read") { model.deviceName == current.name }
        try? await Task.sleep(for: .milliseconds(100))
        #expect(control.ackedRideBatches.isEmpty)
        #expect(model.connectedScope == nil)
        // Browsing is unaffected: the library-first lists still load.
        await waitFor("the lists") { model.loadState == .loaded }
    }

    /// …and the gate re-opens on the next connection whose identity read
    /// succeeds — the closure is per-connection, not sticky.
    @Test func ackReopensOnTheNextSuccessfulIdentityRead() async {
        let library = InMemoryLibraryStore()
        let (model, control) = makeModel(library: library)
        let full = control.deviceInfo
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(3), scope: scope(of: control)))
        control.deviceInfo = DeviceInfo(
            name: full.name, firmwareVersion: full.firmwareVersion,
            hardwareVersion: full.hardwareVersion, serial: full.serial,
            protocolVersion: full.protocolVersion, storeEpoch: nil
        )

        model.start()
        await waitFor("the epoch-less identity read") { model.deviceName == full.name }
        try? await Task.sleep(for: .milliseconds(100))
        #expect(control.ackedRideBatches.isEmpty)

        // The next connection reads a whole identity → the ack fires.
        control.deviceInfo = full
        control.connection = .disconnected
        await waitFor("the drop") { model.connection == .disconnected }
        control.connection = .connected
        await waitFor("the reconnect ack") { !control.ackedRideBatches.isEmpty }
        #expect(model.connectedScope == scope(of: control))
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
