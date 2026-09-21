import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The Settings model driven through `MockTransport`: identity and status load, the rename
/// (config write, app-side propagation, link-bound guard), and forget (bond cleared, host told).
@MainActor
final class SettingsModelTests: XCTestCase {
    private func makeModel(
        _ scenario: Scenario,
        onDeviceRenamed: @escaping (String) -> Void = { _ in },
        onForget: @escaping () -> Void = {}
    ) -> (SettingsModel, MockControl) {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        let model = SettingsModel(
            transport: MockTransport(control: control),
            bondStore: MockBondStore(control: control),
            onDeviceRenamed: onDeviceRenamed,
            onForget: onForget
        )
        return (model, control)
    }

    // MARK: Identity and status

    func testIdentityAndStatusLoadFromTheDevice() async throws {
        let (model, _) = makeModel(.happyPath)
        model.start()

        try await waitFor("identity", timeout: .seconds(5)) { model.deviceName == "Trailhead" }
        try await waitFor("battery", timeout: .seconds(5)) { model.battery == 82 }
        try await waitFor("connection", timeout: .seconds(5)) { model.connection == .connected }
        XCTAssertEqual(model.statusLine, "Connected · 82%")
        XCTAssertEqual(model.firmwareDisplay, "v0.4.2")
        // The row states the version; the firmware screen states the comparison.
        XCTAssertEqual(model.firmwareLine, "v0.4.2")
        XCTAssertTrue(model.canRename)
    }

    func testDegradedLinkDimsRenameAndSaysWhy() async throws {
        let (model, _) = makeModel(.outOfRange)
        model.start()

        try await waitFor("degraded state", timeout: .seconds(5)) { model.connection == .outOfRange }
        XCTAssertEqual(model.statusLine, "Out of range")
        XCTAssertFalse(model.canRename, "H3 is a config write — link-bound (S4 rule)")
        XCTAssertFalse(model.rename(to: "Summit"))
    }

    // MARK: Rename

    func testRenameWritesConfigAndPropagates() async throws {
        var renamedTo: String?
        let (model, control) = makeModel(.happyPath, onDeviceRenamed: { renamedTo = $0 })
        model.start()
        try await waitFor("identity", timeout: .seconds(5)) { model.deviceName == "Trailhead" }

        XCTAssertTrue(model.rename(to: "  Summit  "))

        XCTAssertEqual(model.deviceName, "Summit", "trimmed name shows at once")
        XCTAssertEqual(renamedTo, "Summit", "the host callback refreshes the top bar")
        // The rename rides the Config blob to the device; the mock reflects it into its identity.
        try await waitFor("config write lands", timeout: .seconds(5)) { control.deviceInfo.name == "Summit" }
        // The bond record greets with the new name on the next launch.
        XCTAssertEqual(MockBondStore(control: control).load()?.deviceName, "Summit")
        XCTAssertFalse(model.renameWriteFailed)
    }

    /// Rename is link-bound: a fully dropped link rejects it too, before any optimistic state moves.
    func testRenameWhileDisconnectedReturnsFalse() async throws {
        let (model, control) = makeModel(.happyPath)
        model.start()
        try await waitFor("identity", timeout: .seconds(5)) { model.deviceName == "Trailhead" }

        control.connection = .disconnected
        try await waitFor("link down", timeout: .seconds(5)) { model.connection == .disconnected }

        XCTAssertFalse(model.rename(to: "Summit"))
        XCTAssertEqual(model.deviceName, "Trailhead")
        XCTAssertFalse(model.renameWriteFailed)
    }

    /// When the `writeConfig` leg fails the phone keeps the optimistic name, the one-shot flag
    /// drives the toast, and the bond record keeps the desired name so the reconcile pass converges.
    func testRenameWriteFailureSetsTheFlagAndKeepsTheOptimisticName() async throws {
        let transport = ConfigSpyTransport(config: DeviceConfig(name: "Trailhead"))
        let bondStore = RecordingBondStore(BondRecord(deviceName: "Trailhead"))
        let model = SettingsModel(transport: transport, bondStore: bondStore)
        model.start()
        try await waitFor("connected", timeout: .seconds(5)) { model.connection == .connected }

        transport.failNextWrite()
        XCTAssertTrue(model.rename(to: "Summit"))

        try await waitFor("failure flag", timeout: .seconds(5)) { model.renameWriteFailed }
        XCTAssertEqual(model.deviceName, "Summit", "the rename stays optimistic")
        XCTAssertEqual(transport.config.name, "Trailhead", "the device never got it")
        XCTAssertEqual(bondStore.load()?.deviceName, "Summit", "the desired name survives")

        // "It'll retry next time you connect": the reconcile pass converges.
        await DeviceNameReconciler(transport: transport, bondStore: bondStore).reconcile()
        XCTAssertEqual(transport.config.name, "Summit")
    }

    /// The armed one-shot failure hits the rename's `readConfig` leg, the first op: same flag,
    /// same heal, with `MockBondStore` serving the diverged desired name.
    func testRenameReadFailureFlagsAndReconcileHealsThroughTheMock() async throws {
        let (model, control) = makeModel(.happyPath)
        model.start()
        try await waitFor("identity", timeout: .seconds(5)) { model.deviceName == "Trailhead" }

        control.failNextOp(.readFailed)
        XCTAssertTrue(model.rename(to: "Summit"))

        try await waitFor("failure flag", timeout: .seconds(5)) { model.renameWriteFailed }
        XCTAssertEqual(control.deviceInfo.name, "Trailhead", "the device never got it")
        XCTAssertEqual(MockBondStore(control: control).load()?.deviceName, "Summit")

        let reconciler = DeviceNameReconciler(
            transport: MockTransport(control: control),
            bondStore: MockBondStore(control: control)
        )
        await reconciler.reconcile()
        XCTAssertEqual(control.fixtures.config.name, "Summit")
        XCTAssertEqual(control.deviceInfo.name, "Summit")
    }

    func testRenameCapsOverLongNamesAtTheS0Limit() async throws {
        let (model, control) = makeModel(.happyPath)
        model.start()
        try await waitFor("identity", timeout: .seconds(5)) { model.deviceName == "Trailhead" }

        // 40 three-byte scalars are 120 UTF-8 bytes. The cap lands on a Character boundary at
        // the 16 that fit in 48 bytes, so the app-side name and the stored name agree.
        XCTAssertTrue(model.rename(to: String(repeating: "名", count: 40)))
        XCTAssertEqual(model.deviceName, String(repeating: "名", count: 16))
        XCTAssertLessThanOrEqual(model.deviceName.utf8.count, DeviceConfig.maxNameUTF8Bytes)
        try await waitFor("config write lands", timeout: .seconds(5)) { control.deviceInfo.name == model.deviceName }
    }

    func testRenameRejectsEmptyNames() async throws {
        let (model, control) = makeModel(.happyPath)
        model.start()
        try await waitFor("identity", timeout: .seconds(5)) { model.deviceName == "Trailhead" }

        XCTAssertFalse(model.rename(to: "   "))
        XCTAssertEqual(model.deviceName, "Trailhead")
        XCTAssertEqual(control.deviceInfo.name, "Trailhead")
    }

    // MARK: Stream lifecycle

    /// The state and battery streams never finish, and RootView makes a fresh model per Settings
    /// push, so the loops must not retain the model past its screen.
    func testStreamTasksDoNotRetainTheModel() async throws {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        weak var leaked: SettingsModel?
        do {
            let model = SettingsModel(
                transport: MockTransport(control: control),
                bondStore: MockBondStore(control: control)
            )
            model.start()
            try await waitFor("streams running", timeout: .seconds(5)) { model.connection == .connected }
            leaked = model
        }
        // The last strong reference is gone. Push an event through the still-open streams: a
        // strongly-capturing loop would show up as a live reference.
        control.connection = .outOfRange
        for _ in 0..<10 { await Task.yield() }
        XCTAssertNil(leaked, "the stream loops must hold the model weakly")
    }

    // MARK: Forget

    func testForgetWhileConnectedDissolvesTheDeviceBondThenClears() async throws {
        // A connected forget first tells the device to dissolve its side of the bond, so
        // re-pairing is not wedged by reject-when-bonded, and only then clears the phone's record.
        var forgetFired = false
        let (model, control) = makeModel(.happyPath, onForget: { forgetFired = true })
        model.start()
        try await waitFor("connected", timeout: .seconds(5)) { model.connection == .connected }

        model.forget()

        try await waitFor("device bond dissolved", timeout: .seconds(5)) { control.forgetBondCount == 1 }
        try await waitFor("bond record cleared", timeout: .seconds(5)) { !control.bonded }
        try await waitFor("host signaled", timeout: .seconds(5)) { forgetFired }
    }

    func testConnectedForgetMessageDropsTheDeviceStep() async throws {
        let (model, _) = makeModel(.happyPath)
        model.start()
        try await waitFor("connected", timeout: .seconds(5)) { model.connection == .connected }
        XCTAssertFalse(
            model.forgetMessage.contains("Forget phone"),
            "connected: the app dissolves the device bond, so no device step in the copy"
        )
    }

    func testOfflineForgetClearsWithoutBlockingOrCommandingTheDevice() async throws {
        // The device is unreachable, so no `forgetBond` is sent; the forget still clears the
        // record at once, and the copy keeps the Forget-phone-on-device guidance.
        var forgetFired = false
        let (model, control) = makeModel(.happyPath, onForget: { forgetFired = true })
        model.start()
        try await waitFor("connected", timeout: .seconds(5)) { model.connection == .connected }
        control.connection = .disconnected
        try await waitFor("link down", timeout: .seconds(5)) { model.connection == .disconnected }

        model.forget()

        // Cleared synchronously on the offline path (no await on a device command).
        XCTAssertFalse(control.bonded, "the bond record is gone — next launch pairs")
        XCTAssertTrue(forgetFired, "the host drops the launch flow back to D1")
        XCTAssertEqual(control.forgetBondCount, 0, "offline: never commands the unreachable device")
        XCTAssertTrue(
            model.forgetMessage.contains("Forget phone"),
            "offline: keep the Forget-phone-on-device guidance"
        )
    }

    func testConnectedForgetStillClearsWhenTheCommandFails() async throws {
        // The device may not answer `forgetBond`: a timeout, or the link dropping as it acks. An
        // armed one-shot fault stands in for that; the forget must still clear and signal the host.
        var forgetFired = false
        let (model, control) = makeModel(.happyPath, onForget: { forgetFired = true })
        model.start()
        try await waitFor("connected", timeout: .seconds(5)) { model.connection == .connected }
        control.failNextOp(.writeFailed)

        model.forget()

        try await waitFor("bond record cleared despite the failed command", timeout: .seconds(5)) { !control.bonded }
        try await waitFor("host signaled", timeout: .seconds(5)) { forgetFired }
    }

    func testConnectedForgetClearsEvenIfTheModelDiesDuringTheAckWait() async throws {
        // The `forgetBond` command is sent the moment `forget()` runs, so if the model
        // deallocates during the ack window the local clear must still happen: a surviving
        // BondRecord would reconnect bonded against a device in open pairing. The forget task
        // captures the bond store and `onForget`, never self.
        var forgetFired = false
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        var model: SettingsModel? = SettingsModel(
            transport: MockTransport(control: control),
            bondStore: MockBondStore(control: control),
            onForget: { forgetFired = true }
        )
        model?.start()
        try await waitFor("connected", timeout: .seconds(5)) { model?.connection == .connected }
        control.latency = .milliseconds(200) // hold the ack window open

        model?.forget()
        model = nil // the Settings screen pops mid-wait

        try await waitFor("device bond dissolved", timeout: .seconds(5)) { control.forgetBondCount == 1 }
        try await waitFor("bond record cleared despite the dead model", timeout: .seconds(5)) { !control.bonded }
        try await waitFor("host signaled", timeout: .seconds(5)) { forgetFired }
    }
}
