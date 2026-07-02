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
    }

    func testRenameRejectsEmptyNames() async {
        let (model, control) = makeModel(.happyPath)
        model.start()
        await waitFor("identity") { model.deviceName == "Trailhead" }

        XCTAssertFalse(model.rename(to: "   "))
        XCTAssertEqual(model.deviceName, "Trailhead")
        XCTAssertEqual(control.deviceInfo.name, "Trailhead")
    }

    // MARK: Forget (H2)

    func testForgetClearsTheBondAndSignalsTheHost() async {
        var forgetFired = false
        let (model, control) = makeModel(.happyPath, onForget: { forgetFired = true })
        model.start()
        await waitFor("identity") { model.deviceName == "Trailhead" }

        model.forget()

        XCTAssertFalse(control.bonded, "the bond record is gone — next launch pairs")
        XCTAssertTrue(forgetFired, "the host drops the launch flow back to D1")
    }
}
