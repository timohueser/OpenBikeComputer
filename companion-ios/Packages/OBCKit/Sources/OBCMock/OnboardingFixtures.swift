#if DEBUG
import Foundation
import OBCDomain
import OBCTransport

/// Named first-use branches exercise the real pairing and firmware models with local data.
public enum OnboardingFixtures {
    static func configure(_ control: MockControl) {
        switch control.scenario {
        case .onboardingNearby:
            control.pairingDevices = [
                PairingDevice(id: UUID(uuidString: "00000000-0000-0000-0000-000000000001")!, name: "OBC-7A2F"),
                PairingDevice(id: UUID(uuidString: "00000000-0000-0000-0000-000000000002")!, name: "OBC-9C41")
            ]
        case .onboardingUpdate, .onboardingUpdateNeeded:
            let info = control.deviceInfo
            control.deviceInfo = DeviceInfo(
                name: info.name, firmwareVersion: "0.4.0", hardwareVersion: info.hardwareVersion,
                serial: info.serial,
                protocolVersion: control.scenario == .onboardingUpdateNeeded ? OBCProtocol.version - 1 : OBCProtocol.version,
                storeID: info.storeID, obcmVersion: info.obcmVersion)
        default: break
        }
    }

    /// Both the cached offer and an explicit recheck/download stay in memory. The reserved URLs
    /// identify fixture objects; no request leaves this fetcher, and no release-notes link is shown.
    public static func updateChecker(for scenario: Scenario) -> UpdateChecker? {
        guard scenario == .onboardingUpdate || scenario == .onboardingUpdateNeeded else { return nil }
        let payload = SampleFirmwareFile.container(version: "0.5.0")
        let release = FirmwareRelease(
            version: "0.5.0", bytes: payload.count, sha256: UpdateChecker.sha256Hex(payload),
            url: URL(string: "https://example.invalid/onboarding/UPDATE.BIN")!)
        let fetcher = OnboardingFirmwareFetcher(release: release, payload: payload)
        return UpdateChecker(
            manifestURL: OnboardingFirmwareFetcher.manifestURL,
            fetcher: fetcher,
            store: InMemoryUpdateCheckStore(record: UpdateCheckRecord(release: release, checkedAt: Date())))
    }
}

private struct OnboardingFirmwareFetcher: ManifestFetching {
    static let manifestURL = URL(string: "https://example.invalid/onboarding/manifest.json")!
    let release: FirmwareRelease
    let payload: Data

    func get(_ url: URL) async throws -> (status: Int, body: Data) {
        if url == release.url { return (200, payload) }
        if url == Self.manifestURL { return (200, try JSONEncoder().encode(release)) }
        return (404, Data())
    }
}
#endif
