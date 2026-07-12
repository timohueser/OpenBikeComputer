import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// B8 acceptance, host-side: the Settings model driven through `MockTransport`
/// — identity/status load, the H3 rename (config write + app-side propagation
/// + the S4 link-bound guard), and the H2 forget (bond cleared, host signaled).
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

    // MARK: Identity + status (the G device row)

    func testIdentityAndStatusLoadFromTheDevice() async {
        let (model, _) = makeModel(.happyPath)
        model.start()

        await waitFor("identity") { model.deviceName == "Trailhead" }
        await waitFor("battery") { model.battery == 82 }
        await waitFor("connection") { model.connection == .connected }
        XCTAssertEqual(model.statusLine, "Connected · 82%")
        XCTAssertEqual(model.firmwareDisplay, "v0.4.2")
        XCTAssertEqual(model.firmwareLine, "v0.4.2 · latest")
        XCTAssertTrue(model.canRename)
    }

    func testDegradedLinkDimsRenameAndSaysWhy() async {
        let (model, _) = makeModel(.outOfRange)
        model.start()

        await waitFor("degraded state") { model.connection == .outOfRange }
        XCTAssertEqual(model.statusLine, "Out of range")
        XCTAssertFalse(model.canRename, "H3 is a config write — link-bound (S4 rule)")
        XCTAssertFalse(model.rename(to: "Summit"))
    }

    // MARK: Rename (H3)

    func testRenameWritesConfigAndPropagates() async {
        var renamedTo: String?
        let (model, control) = makeModel(.happyPath, onDeviceRenamed: { renamedTo = $0 })
        model.start()
        await waitFor("identity") { model.deviceName == "Trailhead" }

        XCTAssertTrue(model.rename(to: "  Summit  "))

        XCTAssertEqual(model.deviceName, "Summit", "trimmed name shows at once")
        XCTAssertEqual(renamedTo, "Summit", "the host callback refreshes the top bar")
        // Delta 1: the rename rides the Config blob to the device — the mock
        // reflects it into its served identity.
        await waitFor("config write lands") { control.deviceInfo.name == "Summit" }
        // The bond record greets with the new name on the next launch.
        XCTAssertEqual(MockBondStore(control: control).load()?.deviceName, "Summit")
        // The write landed → nothing to surface, nothing to reconcile.
        XCTAssertFalse(model.renameWriteFailed)
    }

    /// #361 pin: rename is link-bound — a fully dropped link rejects it too
    /// (not only `.outOfRange`), before any optimistic state moves.
    func testRenameWhileDisconnectedReturnsFalse() async {
        let (model, control) = makeModel(.happyPath)
        model.start()
        await waitFor("identity") { model.deviceName == "Trailhead" }

        control.connection = .disconnected
        await waitFor("link down") { model.connection == .disconnected }

        XCTAssertFalse(model.rename(to: "Summit"))
        XCTAssertEqual(model.deviceName, "Trailhead")
        XCTAssertFalse(model.renameWriteFailed)
    }

    /// #361: a rename whose `writeConfig` leg fails — the phone keeps the
    /// optimistic name, the one-shot flag drives the toast, and the *bond
    /// record* keeps the desired name so the reconcile pass can converge.
    func testRenameWriteFailureSetsTheFlagAndKeepsTheOptimisticName() async {
        let transport = ConfigSpyTransport(config: DeviceConfig(name: "Trailhead"))
        let bondStore = RecordingBondStore(BondRecord(deviceName: "Trailhead"))
        let model = SettingsModel(transport: transport, bondStore: bondStore)
        model.start()
        await waitFor("connected") { model.connection == .connected }

        transport.failNextWrite()
        XCTAssertTrue(model.rename(to: "Summit"))

        await waitFor("failure flag") { model.renameWriteFailed }
        XCTAssertEqual(model.deviceName, "Summit", "the rename stays optimistic")
        XCTAssertEqual(transport.config.name, "Trailhead", "the device never got it")
        XCTAssertEqual(bondStore.load()?.deviceName, "Summit", "the desired name survives")

        // "It'll retry next time you connect": the reconcile pass converges.
        await DeviceNameReconciler(transport: transport, bondStore: bondStore).reconcile()
        XCTAssertEqual(transport.config.name, "Summit")
    }

    /// #361, through the full mock wiring: the armed one-shot failure hits the
    /// rename's `readConfig` leg (the first op) — same flag, same heal, and
    /// `MockBondStore` serves the diverged desired name the way the real
    /// `UserDefaultsBondStore` would.
    func testRenameReadFailureFlagsAndReconcileHealsThroughTheMock() async {
        let (model, control) = makeModel(.happyPath)
        model.start()
        await waitFor("identity") { model.deviceName == "Trailhead" }

        control.failNextOp(.readFailed)
        XCTAssertTrue(model.rename(to: "Summit"))

        await waitFor("failure flag") { model.renameWriteFailed }
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

    func testRenameCapsOverLongNamesAtTheS0Limit() async {
        let (model, control) = makeModel(.happyPath)
        model.start()
        await waitFor("identity") { model.deviceName == "Trailhead" }

        // 40 × 3-byte scalars = 120 UTF-8 bytes; the cap lands on a Character
        // boundary at the 16 that fit in 48 B — so the app-side name and the
        // device's stored name agree, and the config blob can't be corrupted.
        XCTAssertTrue(model.rename(to: String(repeating: "名", count: 40)))
        XCTAssertEqual(model.deviceName, String(repeating: "名", count: 16))
        XCTAssertLessThanOrEqual(model.deviceName.utf8.count, DeviceConfig.maxNameUTF8Bytes)
        await waitFor("config write lands") { control.deviceInfo.name == model.deviceName }
    }

    func testRenameRejectsEmptyNames() async {
        let (model, control) = makeModel(.happyPath)
        model.start()
        await waitFor("identity") { model.deviceName == "Trailhead" }

        XCTAssertFalse(model.rename(to: "   "))
        XCTAssertEqual(model.deviceName, "Trailhead")
        XCTAssertEqual(control.deviceInfo.name, "Trailhead")
    }

    // MARK: Stream lifecycle (#356)

    /// The state/battery streams never finish, and RootView makes a fresh model
    /// per Settings push — the loops must not retain the model past its screen.
    func testStreamTasksDoNotRetainTheModel() async {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        weak var leaked: SettingsModel?
        do {
            let model = SettingsModel(
                transport: MockTransport(control: control),
                bondStore: MockBondStore(control: control)
            )
            model.start()
            await waitFor("streams running") { model.connection == .connected }
            leaked = model
        }
        // The model's last strong ref is gone; push an event through the still-
        // open streams so a strongly-capturing loop would show up as a live ref.
        control.connection = .outOfRange
        for _ in 0..<10 { await Task.yield() }
        XCTAssertNil(leaked, "the stream loops must hold the model weakly")
    }

    // MARK: Forget (H2)

    func testForgetWhileConnectedDissolvesTheDeviceBondThenClears() async {
        // #756: a connected forget first tells the device to dissolve its side of
        // the bond (so re-pairing isn't wedged by reject-when-bonded), THEN clears
        // the phone's record and drops the link.
        var forgetFired = false
        let (model, control) = makeModel(.happyPath, onForget: { forgetFired = true })
        model.start()
        await waitFor("connected") { model.connection == .connected }

        model.forget()

        await waitFor("device bond dissolved") { control.forgetBondCount == 1 }
        await waitFor("bond record cleared") { !control.bonded }
        await waitFor("host signaled") { forgetFired }
    }

    func testConnectedForgetMessageDropsTheDeviceStep() async {
        let (model, _) = makeModel(.happyPath)
        model.start()
        await waitFor("connected") { model.connection == .connected }
        XCTAssertFalse(
            model.forgetMessage.contains("Forget phone"),
            "connected: the app dissolves the device bond, so no device step in the copy"
        )
    }

    func testOfflineForgetClearsWithoutBlockingOrCommandingTheDevice() async {
        // Offline: the device is unreachable, so no `forgetBond` is sent (it would
        // only throw), and the forget still clears the record immediately — exactly
        // the prior behaviour. The copy keeps the Forget-phone-on-device guidance.
        var forgetFired = false
        let (model, control) = makeModel(.happyPath, onForget: { forgetFired = true })
        model.start()
        await waitFor("connected") { model.connection == .connected }
        control.connection = .disconnected
        await waitFor("link down") { model.connection == .disconnected }

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

    func testConnectedForgetStillClearsWhenTheCommandFails() async {
        // Best-effort: the device may not answer `forgetBond` (a genuine timeout,
        // or the link dropping the instant it acks). An armed one-shot fault stands
        // in for that — the forget must still clear the record and signal the host.
        var forgetFired = false
        let (model, control) = makeModel(.happyPath, onForget: { forgetFired = true })
        model.start()
        await waitFor("connected") { model.connection == .connected }
        control.failNextOp(.writeFailed)

        model.forget()

        await waitFor("bond record cleared despite the failed command") { !control.bonded }
        await waitFor("host signaled") { forgetFired }
    }
}
