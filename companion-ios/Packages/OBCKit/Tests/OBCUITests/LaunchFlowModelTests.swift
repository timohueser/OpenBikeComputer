import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// B2 acceptance, host-side: the launch/pairing state machine driven through
/// `MockTransport` scenarios — every design branch (A, D1–D5, H7, H8) plus the
/// non-blocking guarantees (out of range → main; a silent device caps the A
/// grace onto connect-failed, never a forever-spinner).
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

    // MARK: The launch branch

    func testFirstRunBranchesToPairIntro() {
        let (model, _) = makeModel(.noDevice)
        model.start()
        XCTAssertEqual(model.phase, .pairIntro)
    }

    /// H2 (B8): forgetting the device drops the flow back to the D1 prompt.
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
        // No connect attempt — the degraded link is the S4 banner's story.
        XCTAssertEqual(control.connection, .outOfRange)
    }

    func testBondedConnectFailureStillLandsOnMain() async throws {
        let (model, control) = makeModel(.happyPath)
        control.connection = .disconnected
        control.radio = .off  // connect() will throw — must degrade, not error
        model.start()
        try await waitFor("main", timeout: .seconds(5)) { model.phase == .main }
    }

    /// The device never answers (asleep / out of range): the grace window
    /// expires onto the connect-failed screen — never a forever-spinner — and
    /// "Go to routes" still reaches the library.
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

    /// Connect-failed "Try again" re-enters A; with the device now answering,
    /// the retry lands on main.
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

    /// The background attempt outliving the grace window still counts: when the
    /// link comes up while the connect-failed screen is showing, the flow
    /// advances to main on its own.
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

    /// #297: a declined passkey is a *gated* failure, so it surfaces on the row tap
    /// (`confirmPairing`), not during the un-gated scan — the D2 row appears first.
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

    /// #461: an **already-bonded** refusal is indistinguishable on the wire from a
    /// declined passkey (spec §8 / `OBCProtocol.md`) — the device suppresses its
    /// passkey and drops the link, and CoreBluetooth surfaces only a generic
    /// `DeviceError.pairingFailed`. So there is nothing to classify: it must land
    /// on the *same* `.pairFailed(.rejected)` screen the declined passkey does,
    /// which is why that one screen carries the combined copy. This pins that the
    /// generic transport pairing failure maps to `.rejected` (not a new case).
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

    // MARK: D5 copy (#461)

    /// The `.rejected` screen's copy is *combined*: it must name the
    /// already-paired possibility and its recovery (Forget phone on the device)
    /// **without asserting** which failure happened, while still offering the
    /// passkey retry. Copy lives on the model so it's testable without rendering.
    func testRejectedCopyCoversBothPasskeyAndAlreadyBonded() {
        let reason = LaunchFlowModel.PairingFailure.rejected.reason

        // Doesn't assert a single cause — offers the passkey retry AND names the
        // already-paired possibility conditionally ("If … / If …").
        XCTAssertTrue(reason.contains("passkey"), "must mention the passkey path")
        XCTAssertTrue(
            reason.localizedCaseInsensitiveContains("already paired to another phone"),
            "must name the already-bonded possibility"
        )
        XCTAssertTrue(
            reason.contains("If the passkey was wrong") && reason.contains("If the device is already paired"),
            "must present both as possibilities, not assert one"
        )

        // Names the concrete bonded-case recovery (#455): Forget phone on the device.
        XCTAssertTrue(
            reason.contains("Forget phone"),
            "must point at Forget phone as the re-pair recovery"
        )

        XCTAssertEqual(LaunchFlowModel.PairingFailure.rejected.title, "Pairing didn't finish")
    }

    /// The timeout variant is untouched — still its own scan copy, distinct from
    /// the rejected combined copy.
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
        model.confirmPairing()  // #297: the decline lands on the gated row tap
        try await waitFor("D5", timeout: .seconds(5)) { model.phase == .pairFailed(.rejected) }

        model.showPairingHelp()
        XCTAssertEqual(model.phase, .pairIntro)
    }
}
