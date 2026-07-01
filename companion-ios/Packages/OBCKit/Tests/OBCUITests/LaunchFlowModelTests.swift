import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// B2 acceptance, host-side: the launch/pairing state machine driven through
/// `MockTransport` scenarios — every design branch (A, D1–D5, H7, H8) plus the
/// non-blocking guarantees (out of range → main, grace-capped connect).
@MainActor
final class LaunchFlowModelTests: XCTestCase {
    /// Short pacing so the machine's timers fire in test time, but with enough
    /// slack that they never race a healthy mock op.
    private static let fastTiming = LaunchFlowModel.Timing(
        connectGrace: .seconds(2),
        scanTimeout: .seconds(2),
        pairingBeat: .milliseconds(10)
    )

    private func makeModel(
        _ scenario: Scenario,
        timing: LaunchFlowModel.Timing = fastTiming
    ) -> (LaunchFlowModel, MockControl) {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        let model = LaunchFlowModel(
            transport: MockTransport(control: control),
            bondStore: MockBondStore(control: control),
            timing: timing
        )
        return (model, control)
    }

    /// Poll until `condition` holds (the phases move on free-running tasks).
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

    // MARK: The launch branch

    func testFirstRunBranchesToPairIntro() {
        let (model, _) = makeModel(.noDevice)
        model.start()
        XCTAssertEqual(model.phase, .pairIntro)
    }

    func testBondedColdLaunchShowsConnectingThenMain() async {
        let (model, control) = makeModel(.happyPath)
        control.connection = .disconnected  // cold boot: bonded but link down
        model.start()
        XCTAssertEqual(model.phase, .connecting(deviceName: "Trailhead"))
        await waitFor("main") { model.phase == .main }
        XCTAssertEqual(control.connection, .connected)
    }

    func testBondedLaunchAlreadyConnectedGoesStraightToMain() async {
        let (model, _) = makeModel(.happyPath)
        model.start()
        await waitFor("main") { model.phase == .main }
    }

    func testOutOfRangeLandsOnMainNotAnError() async {
        let (model, control) = makeModel(.outOfRange)
        model.start()
        await waitFor("main") { model.phase == .main }
        // No connect attempt — the degraded link is the S4 banner's story.
        XCTAssertEqual(control.connection, .outOfRange)
    }

    func testBondedConnectFailureStillLandsOnMain() async {
        let (model, control) = makeModel(.happyPath)
        control.connection = .disconnected
        control.radio = .off  // connect() will throw — must degrade, not error
        model.start()
        await waitFor("main") { model.phase == .main }
    }

    func testConnectGraceCapsTheConnectingState() async {
        let (model, control) = makeModel(
            .happyPath,
            timing: .init(connectGrace: .milliseconds(50), scanTimeout: .seconds(2), pairingBeat: .zero)
        )
        control.connection = .disconnected
        control.latency = .seconds(30)  // connect() would block far past the cap
        model.start()
        await waitFor("main despite a hung connect") { model.phase == .main }
    }

    // MARK: The pairing flow

    func testPairingHappyPathThroughAllScreens() async {
        let (model, control) = makeModel(.noDevice)
        model.start()
        XCTAssertEqual(model.phase, .pairIntro)

        model.startPairing()
        guard case .scanning = model.phase else {
            return XCTFail("expected scanning, got \(model.phase)")
        }
        await waitFor("discovered row") {
            model.phase == .scanning(discovered: .init(name: "Trailhead"))
        }
        XCTAssertEqual(
            LaunchFlowModel.DiscoveredDevice(name: "Trailhead").advertisedName,
            "OBC-Trailhead"
        )

        model.confirmPairing()
        XCTAssertEqual(model.phase, .pairing)
        await waitFor("paired") { model.phase == .paired(deviceName: "Trailhead") }
        XCTAssertTrue(control.bonded, "pairing success must record the bond")

        model.finishPairing()
        XCTAssertEqual(model.phase, .main)
    }

    func testPairingTimeoutShowsD5AndRetryLoopsToScanning() async {
        let (model, _) = makeModel(.pairingTimeout)
        model.start()
        model.startPairing()
        await waitFor("D5 timeout") { model.phase == .pairFailed(.timeout) }

        model.retryPairing()
        guard case .scanning = model.phase else {
            return XCTFail("retry must loop back to scanning, got \(model.phase)")
        }
        await waitFor("D5 again") { model.phase == .pairFailed(.timeout) }
    }

    func testPairingRejectedShowsD5RejectedVariant() async {
        let (model, _) = makeModel(.pairingRejected)
        model.start()
        model.startPairing()
        await waitFor("D5 rejected") { model.phase == .pairFailed(.rejected) }
    }

    func testScanWindowExpiryIsATimeout() async {
        let (model, control) = makeModel(
            .noDevice,
            timing: .init(connectGrace: .seconds(2), scanTimeout: .milliseconds(50), pairingBeat: .zero)
        )
        control.latency = .seconds(30)  // the mock never "finds" the device in time
        model.start()
        model.startPairing()
        await waitFor("scan-window timeout") { model.phase == .pairFailed(.timeout) }
    }

    func testBluetoothOffShowsH8AndLibraryStaysReachable() async {
        let (model, _) = makeModel(.bluetoothOff)
        model.start()
        model.startPairing()
        await waitFor("H8") { model.phase == .radioBlocked(.off) }

        model.browseLibrary()
        XCTAssertEqual(model.phase, .main)
    }

    func testPermissionDeniedShowsH7State() async {
        let (model, _) = makeModel(.permissionDenied)
        model.start()
        model.startPairing()
        await waitFor("H7") { model.phase == .radioBlocked(.denied) }
    }

    func testCancelScanningStepsBackAndDropsTheLink() async {
        let (model, control) = makeModel(.noDevice)
        control.latency = .milliseconds(200)
        model.start()
        model.startPairing()
        model.cancelScanning()
        XCTAssertEqual(model.phase, .pairIntro)
        await waitFor("link down") { control.connection == .disconnected }
    }

    func testPairingHelpReturnsToTheIntroSteps() async {
        let (model, _) = makeModel(.pairingRejected)
        model.start()
        model.startPairing()
        await waitFor("D5") { model.phase == .pairFailed(.rejected) }

        model.showPairingHelp()
        XCTAssertEqual(model.phase, .pairIntro)
    }
}
