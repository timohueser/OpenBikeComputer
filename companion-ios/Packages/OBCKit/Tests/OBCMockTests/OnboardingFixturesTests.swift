import Foundation
import Testing
import OBCDomain
import OBCTransport
import OBCMock

struct OnboardingFixturesTests {
    @Test func nearbyScenarioSelectsTheNamedPeer() async throws {
        let control = MockControl(scenario: .onboardingNearby)
        control.latency = .zero
        let transport = MockTransport(control: control)
        let candidates = try await transport.scanForPairing()
        #expect(candidates.map(\.name) == ["OBC-7A2F", "OBC-9C41"])
        #expect(Set(candidates.map(\.id)).count == 2)
        try await transport.discover(try #require(candidates.last))
        try await transport.authenticate()
        #expect(try await transport.deviceInfo().name == "OBC-9C41")
        control.apply(.onboarding)
        #expect(control.pairingDevices == nil)
        #expect(try await transport.scanForPairing().map(\.name) == ["OBC-7A2F"])
    }

    @Test(arguments: [Scenario.onboardingUpdate, .onboardingUpdateNeeded])
    func localManifestAndContainerExerciseTheNormalUpdater(_ scenario: Scenario) async throws {
        let control = MockControl(scenario: scenario)
        let expectedProtocol = scenario == .onboardingUpdateNeeded ? OBCProtocol.version - 1 : OBCProtocol.version
        #expect(control.deviceInfo.protocolVersion == expectedProtocol)
        let checker = try #require(OnboardingFixtures.updateChecker(for: scenario))
        let cached = try #require(checker.cachedCheck())
        let release = try #require(cached.release)
        #expect(FirmwareVersion.updateStatus(running: control.deviceInfo.firmwareVersion, latest: release.version) == .available)
        #expect(try await checker.check().release == release)
        let downloaded = try await checker.download(release)
        let staged = try StagedFirmware.validate(downloaded)
        #expect(staged.version == "0.5.0")
    }
}
