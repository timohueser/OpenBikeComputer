import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The launch and pairing state machine driven through `MockTransport` scenarios: every design
/// branch, plus the non-blocking guarantees (out of range lands on main; a silent device caps the
/// grace window onto connect-failed instead of spinning forever).
@MainActor
final class LaunchFlowModelTests: XCTestCase {
    /// Short pacing so the timers fire in test time, with enough slack that they never race a
    /// healthy mock op.
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

    // MARK: The launch branch

    func testFirstRunBranchesToPairIntro() {
        let (model, _) = makeModel(.noDevice)
        model.start()
        XCTAssertEqual(model.phase, .pairIntro)
    }

    func testForgetDeviceReturnsToPairIntro() async throws {
        let (model, _) = makeModel(.happyPath)
        model.start()
        try await waitFor("main", timeout: .seconds(5)) { model.phase == .main }

        model.forgetDevice()
        XCTAssertEqual(model.phase, .pairIntro)
    }

    func testBondedColdLaunchShowsConnectingThenMain() async throws {
        let (model, control) = makeModel(.happyPath)
        control.connection = .disconnected  // cold boot: bonded but link down
        model.start()
        XCTAssertEqual(model.phase, .connecting(deviceName: "Trailhead"))
        try await waitFor("main", timeout: .seconds(5)) { model.phase == .main }
        XCTAssertEqual(control.connection, .connected)
    }

    func testBondedLaunchAlreadyConnectedGoesStraightToMain() async throws {
        let (model, _) = makeModel(.happyPath)
        model.start()
        try await waitFor("main", timeout: .seconds(5)) { model.phase == .main }
    }

    func testOutOfRangeLandsOnMainNotAnError() async throws {
        let (model, control) = makeModel(.outOfRange)
        model.start()
        try await waitFor("main", timeout: .seconds(5)) { model.phase == .main }
        // No connect attempt: the degraded link is the banner's story.
        XCTAssertEqual(control.connection, .outOfRange)
    }

    func testBondedConnectFailureStillLandsOnMain() async throws {
        let (model, control) = makeModel(.happyPath)
        control.connection = .disconnected
        control.radio = .off  // connect() will throw — must degrade, not error
        model.start()
        try await waitFor("main", timeout: .seconds(5)) { model.phase == .main }
    }

    func testConnectGraceExpiryShowsConnectFailedAndRoutesStayReachable() async throws {
        let (model, control) = makeModel(
            .happyPath,
            timing: .init(connectGrace: .milliseconds(50), scanTimeout: .seconds(2), pairingBeat: .zero)
        )
        control.connection = .disconnected
        control.latency = .seconds(30)  // connect() parks far past the cap
        model.start()
        try await waitFor("connect-failed despite a hung connect", timeout: .seconds(5)) {
            model.phase == .connectFailed(deviceName: "Trailhead")
        }

        model.browseLibrary()
        XCTAssertEqual(model.phase, .main)
    }

    func testRetryConnectFromConnectFailedLandsOnMainOnceReachable() async throws {
        let (model, control) = makeModel(
            .happyPath,
            timing: .init(connectGrace: .milliseconds(50), scanTimeout: .seconds(2), pairingBeat: .zero)
        )
        control.connection = .disconnected
        control.latency = .seconds(30)
        model.start()
        try await waitFor("connect-failed", timeout: .seconds(5)) { model.phase == .connectFailed(deviceName: "Trailhead") }

        control.connection = .connected  // the device woke up / came into range
        model.retryConnect()
        XCTAssertEqual(model.phase, .connecting(deviceName: "Trailhead"))
        try await waitFor("main after retry", timeout: .seconds(5)) { model.phase == .main }
    }

    func testLateConnectWhileOnConnectFailedAdvancesToMain() async throws {
        let (model, control) = makeModel(
            .happyPath,
            timing: .init(connectGrace: .milliseconds(50), scanTimeout: .seconds(2), pairingBeat: .zero)
        )
        control.connection = .disconnected
        control.latency = .milliseconds(300)  // slower than the grace, but finite
        model.start()
        try await waitFor("connect-failed first", timeout: .seconds(5)) { model.phase == .connectFailed(deviceName: "Trailhead") }
        try await waitFor("main once the late connect lands", timeout: .seconds(5)) { model.phase == .main }
    }

    // MARK: The pairing flow

    func testPairingHappyPathThroughAllScreens() async throws {
        let (model, control) = makeModel(.noDevice)
        model.start()
        XCTAssertEqual(model.phase, .pairIntro)

        model.startPairing()
        guard case .scanning = model.phase else {
            return XCTFail("expected scanning, got \(model.phase)")
        }
        try await waitFor("discovered row", timeout: .seconds(5)) {
            model.phase == .scanning(discovered: .init(name: "Trailhead"))
        }
        XCTAssertEqual(
            LaunchFlowModel.DiscoveredDevice(name: "Trailhead").advertisedName,
            "OBC-Trailhead"
        )

        model.confirmPairing()
        XCTAssertEqual(model.phase, .pairing)
        try await waitFor("paired", timeout: .seconds(5)) { model.phase == .paired(deviceName: "Trailhead") }
        XCTAssertTrue(control.bonded, "pairing success must record the bond")

        model.finishPairing()
        XCTAssertEqual(model.phase, .main)
    }

    func testPairingTimeoutShowsD5AndRetryLoopsToScanning() async throws {
        let (model, _) = makeModel(.pairingTimeout)
        model.start()
        model.startPairing()
        try await waitFor("D5 timeout", timeout: .seconds(5)) { model.phase == .pairFailed(.timeout) }

        model.retryPairing()
        guard case .scanning = model.phase else {
            return XCTFail("retry must loop back to scanning, got \(model.phase)")
        }
        try await waitFor("D5 again", timeout: .seconds(5)) { model.phase == .pairFailed(.timeout) }
    }

    /// A declined passkey is a gated failure: it surfaces on the row tap, not during the scan.
    func testPairingRejectedShowsD5RejectedVariant() async throws {
        let (model, _) = makeModel(.pairingRejected)
        model.start()
        model.startPairing()
        try await waitFor("discovered row", timeout: .seconds(5)) {
            model.phase == .scanning(discovered: .init(name: "Trailhead"))
        }
        model.confirmPairing()
        XCTAssertEqual(model.phase, .pairing)
        try await waitFor("D5 rejected", timeout: .seconds(5)) { model.phase == .pairFailed(.rejected) }
    }

    /// An already-bonded refusal is indistinguishable on the wire from a declined passkey: the
    /// device drops the link and CoreBluetooth surfaces a generic `DeviceError.pairingFailed`. It
    /// lands on the same `.pairFailed(.rejected)` screen, whose copy covers both.
    func testGenericPairingFailureLandsOnRejectedForBondedCase() async throws {
        let (model, _) = makeModel(.pairingRejected)
        model.start()
        model.startPairing()
        try await waitFor("discovered row", timeout: .seconds(5)) {
            model.phase == .scanning(discovered: .init(name: "Trailhead"))
        }
        model.confirmPairing()
        try await waitFor("generic pairing failure → rejected", timeout: .seconds(5)) {
            model.phase == .pairFailed(.rejected)
        }
    }

    // MARK: The rejected-pairing copy

    /// The `.rejected` copy must offer the code retry and name the already-paired possibility
    /// with its recovery, without asserting which failure happened.
    func testRejectedCopyCoversBothPasskeyAndAlreadyBonded() {
        let reason = LaunchFlowModel.PairingFailure.rejected.reason

        XCTAssertTrue(
            reason.contains("If the code was wrong") && reason.contains("If the OBC is already paired to another phone"),
            "must present both as possibilities, not assert one"
        )

        XCTAssertTrue(
            reason.contains("Forget phone"),
            "must point at Forget phone as the re-pair recovery"
        )

        XCTAssertEqual(LaunchFlowModel.PairingFailure.rejected.title, "Pairing didn't finish")
    }

    func testTimeoutCopyUnchanged() {
        let timeout = LaunchFlowModel.PairingFailure.timeout
        XCTAssertEqual(timeout.title, "Couldn't find your OBC")
        XCTAssertTrue(timeout.reason.contains("scanned for 30 seconds"))
        XCTAssertFalse(timeout.reason.contains("Forget phone"))
    }

    func testScanWindowExpiryIsATimeout() async throws {
        let (model, control) = makeModel(
            .noDevice,
            timing: .init(connectGrace: .seconds(2), scanTimeout: .milliseconds(50), pairingBeat: .zero)
        )
        control.latency = .seconds(30)  // the mock never "finds" the device in time
        model.start()
        model.startPairing()
        try await waitFor("scan-window timeout", timeout: .seconds(5)) { model.phase == .pairFailed(.timeout) }
    }

    func testBluetoothOffShowsH8AndLibraryStaysReachable() async throws {
        let (model, _) = makeModel(.bluetoothOff)
        model.start()
        model.startPairing()
        try await waitFor("H8", timeout: .seconds(5)) { model.phase == .radioBlocked(.off) }

        model.browseLibrary()
        XCTAssertEqual(model.phase, .main)
    }

    func testPermissionDeniedShowsH7State() async throws {
        let (model, _) = makeModel(.permissionDenied)
        model.start()
        model.startPairing()
        try await waitFor("H7", timeout: .seconds(5)) { model.phase == .radioBlocked(.denied) }
    }

    func testCancelScanningStepsBackAndDropsTheLink() async throws {
        let (model, control) = makeModel(.noDevice)
        control.latency = .milliseconds(200)
        model.start()
        model.startPairing()
        model.cancelScanning()
        XCTAssertEqual(model.phase, .pairIntro)
        try await waitFor("link down", timeout: .seconds(5)) { control.connection == .disconnected }
    }

    func testPairingHelpReturnsToTheIntroSteps() async throws {
        let (model, _) = makeModel(.pairingRejected)
        model.start()
        model.startPairing()
        try await waitFor("discovered row", timeout: .seconds(5)) {
            model.phase == .scanning(discovered: .init(name: "Trailhead"))
        }
        model.confirmPairing()  // the decline lands on the gated row tap
        try await waitFor("D5", timeout: .seconds(5)) { model.phase == .pairFailed(.rejected) }

        model.showPairingHelp()
        XCTAssertEqual(model.phase, .pairIntro)
    }
}
