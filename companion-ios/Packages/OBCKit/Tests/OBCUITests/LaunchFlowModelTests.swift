import Foundation
import Testing
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

@MainActor
struct LaunchFlowModelTests {
    private static let timing = LaunchFlowModel.Timing(
        scanTimeout: .seconds(2), pairingBeat: .zero)

    private func make(
        _ scenario: Scenario = .noDevice,
        timing: LaunchFlowModel.Timing = Self.timing,
        pending: @escaping @MainActor () -> Bool = { false }
    ) -> (LaunchFlowModel, MockControl) {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        return (LaunchFlowModel(
            transport: MockTransport(control: control), bondStore: MockBondStore(control: control),
            timing: timing, onboardingPending: pending), control)
    }

    private func wait(until condition: () -> Bool) async throws {
        let deadline = ContinuousClock.now.advanced(by: .seconds(3))
        while !condition(), ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(5)) }
        #expect(condition())
    }

    private func pair(_ model: LaunchFlowModel) async throws {
        model.start()
        model.startPairing()
        model.allowBluetooth()
        try await wait { if case .paired = model.phase { true } else { false } }
    }

    @Test func welcomeAndPermissionPrecedeRadioUse() {
        let (model, control) = make()
        model.start()
        #expect(model.phase == .welcome)
        model.showSwitchOn()
        #expect(model.phase == .pairIntro)
        model.startPairing()
        #expect(model.phase == .bluetoothPermission)
        #expect(control.connection == .disconnected)
        #expect(!control.bonded)
        model.browseLibrary()
        #expect(model.phase == .main)
    }

    @Test func qrSkipsSwitchOnAndPreservesExistingBond() async throws {
        let (fresh, _) = make()
        fresh.openPairingLink()
        #expect(fresh.phase == .bluetoothPermission)
        fresh.start()
        #expect(fresh.phase == .bluetoothPermission)
        let (bonded, control) = make(.happyPath)
        bonded.openPairingLink()
        try await wait { bonded.phase == .main }
        #expect(control.bonded)
        bonded.openPairingLink()
        #expect(bonded.phase == .main)
    }

    @Test(arguments: [
        "http://openbikecomputer.com/app", "https://other.example/app",
        "https://openbikecomputer.com/app/", "https://openbikecomputer.com/app?device=123",
        "https://openbikecomputer.com/app#pair", "https://user@openbikecomputer.com/app",
        "https://openbikecomputer.com:443/app", "file:///app", "https://openbikecomputer.com/application"
    ])
    func unrelatedLinksDoNotStartPairing(_ value: String) throws {
        #expect(!LaunchFlowModel.acceptsPairingLink(try #require(URL(string: value))))
    }

    @Test func fixedLinkIsAccepted() throws {
        #expect(LaunchFlowModel.acceptsPairingLink(try #require(URL(string: "https://openbikecomputer.com/app"))))
    }

    @Test func oneCandidatePairsDirectlyThenHandsOffToSetup() async throws {
        let (model, control) = make()
        try await pair(model)
        #expect(model.phase == .paired(deviceName: "Trailhead"))
        #expect(control.bonded)
        #expect(model.deviceNameDraft == "Trailhead")
        model.finishPairing()
        #expect(model.phase == .setup)
        model.finishSetup()
        #expect(model.phase == .main)
    }

    @Test func multipleCandidatesRequireExactSelection() async throws {
        let (model, control) = make()
        let first = PairingDevice(id: UUID(), name: "OBC-First")
        let second = PairingDevice(id: UUID(), name: "OBC-Second")
        control.pairingDevices = [first, second]
        model.startPairing()
        model.allowBluetooth()
        try await wait { model.phase == .scanning(devices: [first, second]) }
        #expect(!control.bonded)
        model.confirmPairing(PairingDevice(id: UUID(), name: first.name))
        #expect(model.phase == .scanning(devices: [first, second]))
        model.confirmPairing(second)
        model.confirmPairing(first)
        try await wait { model.phase == .paired(deviceName: "OBC-Second") }
        #expect(control.deviceInfo.name == "OBC-Second")
        #expect(control.bondedName == "OBC-Second")
    }

    @Test func namingWritesExistingConfigAndPersistsOnlySuccess() async throws {
        let (model, control) = make()
        try await pair(model)
        let original = control.fixtures.config
        model.deviceNameDraft = "  New bike  "
        model.saveNameAndContinue()
        try await wait { model.phase == .setup }
        var expected = original
        expected.name = "New bike"
        #expect(control.fixtures.config == expected)
        #expect(control.bondedName == "New bike")
    }

    @Test func failedNameCanRetryOrKeepFactoryName() async throws {
        let (model, control) = make()
        try await pair(model)
        control.failNextOp(.writeFailed)
        model.deviceNameDraft = "New name"
        model.saveNameAndContinue()
        try await wait { model.nameSaveError != nil }
        #expect(model.phase == .paired(deviceName: "Trailhead"))
        #expect(control.bondedName == "Trailhead")
        #expect(control.fixtures.config.name == "Trailhead")
        model.saveNameAndContinue()
        try await wait { model.phase == .setup }
        #expect(control.bondedName == "New name")
    }

    @Test func emptyNameDoesNotAdvanceAndKeepingNameMakesNoWrite() async throws {
        let (model, control) = make()
        try await pair(model)
        model.deviceNameDraft = "  "
        model.saveNameAndContinue()
        #expect(model.phase == .paired(deviceName: "Trailhead"))
        #expect(!model.canSaveName)
        control.failNextOp(.writeFailed)
        model.finishPairing()
        #expect(model.phase == .setup)
        #expect(control.bondedName == "Trailhead")
    }

    @Test func unicodeNameFitsDeviceByteLimit() {
        let normalized = DeviceRenaming.normalized("  " + String(repeating: "🚲", count: 30) + "  ")
        #expect(normalized.utf8.count <= DeviceConfig.maxNameUTF8Bytes)
        #expect(!normalized.isEmpty)
        #expect(normalized.allSatisfy { $0 == "🚲" })
    }

    @Test func emptyScanAndRetryUseTimeoutRecovery() async throws {
        let (model, control) = make()
        control.pairingDevices = []
        model.startPairing()
        model.allowBluetooth()
        try await wait { model.phase == .pairFailed(.timeout) }
        control.pairingDevices = nil
        model.retryPairing()
        try await wait { model.phase == .paired(deviceName: "Trailhead") }
    }

    @Test func radioAndPairingFailuresStayActionable() async throws {
        for (scenario, expected) in [
            (Scenario.bluetoothOff, LaunchFlowModel.Phase.radioBlocked(.off)),
            (.permissionDenied, .radioBlocked(.denied)),
            (.pairingRejected, .pairFailed(.rejected))
        ] {
            let (model, _) = make(scenario)
            model.startPairing()
            model.allowBluetooth()
            try await wait { model.phase == expected }
            model.browseLibrary()
            #expect(model.phase == .main)
        }
    }

    @Test func scanTimeoutAndCancellationDoNotLeaveStaleWork() async throws {
        let (model, control) = make(timing: .init(scanTimeout: .milliseconds(30), pairingBeat: .zero))
        control.latency = .seconds(30)
        model.startPairing()
        model.allowBluetooth()
        try await wait { model.phase == .pairFailed(.timeout) }
        model.retryPairing()
        model.cancelScanning()
        #expect(model.phase == .pairIntro)
        control.latency = .zero
        model.startPairing()
        model.allowBluetooth()
        try await wait { model.phase == .paired(deviceName: "Trailhead") }
        #expect(control.connection == .connected)
    }

    @Test func cancellingAuthenticationCannotAdvanceOrReopenLink() async throws {
        let (model, control) = make()
        control.latency = .milliseconds(80)
        model.startPairing()
        model.allowBluetooth()
        try await wait { model.phase == .pairing }
        model.cancelScanning()
        try await Task.sleep(for: .milliseconds(350))
        #expect(model.phase == .pairIntro)
        #expect(control.connection == .disconnected)
        #expect(!control.bonded)
    }

    @Test func incompatibleDeviceStillReachesOptionalSetup() async throws {
        let (model, control) = make()
        control.deviceInfo = DeviceInfo(name: "Trailhead", firmwareVersion: "0.1.0", protocolVersion: 3)
        try await pair(model)
        model.finishPairing()
        #expect(model.phase == .setup)
    }

    @Test func bondedLaunchResumesOnlyPendingSetup() {
        let (normal, _) = make(.happyPath)
        normal.start()
        #expect(normal.phase == .main)
        let (pending, control) = make(.deviceUnreachable, pending: { true })
        control.latency = .seconds(3_600)
        pending.start()
        #expect(pending.phase == .setup)
        pending.finishSetup()
        pending.replaySetup()
        #expect(pending.phase == .setup)
    }

    @Test(arguments: [ConnectionState.disconnected, .connecting, .connected, .outOfRange])
    func bondedLaunchShowsLibraryBeforeAnyLinkWork(_ connection: ConnectionState) {
        let (model, control) = make(.happyPath)
        control.connection = connection
        control.latency = .seconds(3_600)
        model.start()
        #expect(model.phase == .main)
        #expect(control.connection == connection)
        #expect(control.bonded)
    }

    @Test func backgroundReconnectNeverChangesLibraryNavigation() async throws {
        let (model, control) = make(.happyPath)
        control.connection = .disconnected
        let states = MockTransport(control: control).state
        var observed: [ConnectionState] = []
        let watch = Task { for await state in states { observed.append(state) } }
        defer { watch.cancel() }
        model.start()
        #expect(model.phase == .main)
        try await wait { observed.count == 3 }
        #expect(observed == [.disconnected, .connecting, .connected])
        #expect(model.phase == .main)
    }

    @Test func failedBackgroundReconnectKeepsLibraryAndBond() async throws {
        let (model, control) = make(.happyPath)
        control.connection = .disconnected
        control.failNextOp(.readFailed)
        let states = MockTransport(control: control).state
        var observed: [ConnectionState] = []
        let watch = Task { for await state in states { observed.append(state) } }
        defer { watch.cancel() }
        model.start()
        #expect(model.phase == .main)
        try await wait { observed.count == 3 }
        #expect(observed == [.disconnected, .connecting, .disconnected])
        #expect(model.phase == .main)
        #expect(control.bonded)
    }

    @Test func forgetReturnsToWelcome() async throws {
        let (model, _) = make(.happyPath)
        model.start()
        try await wait { model.phase == .main }
        model.forgetDevice()
        #expect(model.phase == .welcome)
    }
}
