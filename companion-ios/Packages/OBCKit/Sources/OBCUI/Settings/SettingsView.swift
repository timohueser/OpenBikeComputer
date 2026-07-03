import SwiftUI
import OBCDomain
import OBCTransport

/// The Settings screen: grouped iOS lists — Device management up top, a
/// firmware/OTA section present but marked coming soon, the clearly-future
/// connected-services seam (disabled), and About. Nothing here implies a
/// cloud or account (the About footer says so in as many words).
public struct SettingsView: View {
    private let model: SettingsModel
    /// Debug-only: the hidden second entry into the mock dev panel — five
    /// taps on the App version row. `nil` in Release wiring, where the
    /// gesture goes nowhere.
    private let onOpenDevPanel: (() -> Void)?

    @State private var renameShown = false
    @State private var renameDraft = ""
    @State private var forgetShown = false
    @State private var versionTaps = 0
    @Environment(\.openURL) private var openURL

    private static let gitHubURL = URL(string: "https://github.com/timohueser/OpenBikeComputer")!

    public init(model: SettingsModel, onOpenDevPanel: (() -> Void)? = nil) {
        self.model = model
        self.onOpenDevPanel = onOpenDevPanel
    }

    public var body: some View {
        ScrollView {
            VStack(spacing: 26) {
                deviceGroup
                firmwareGroup
                servicesGroup
                aboutGroup
            }
            .padding(.horizontal, 20)
            .padding(.top, 18)
            .padding(.bottom, 30)
        }
        .background(OBCTheme.parchment.ignoresSafeArea())
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("settings.screen")
        .navigationTitle("Settings")
        #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
        #endif
        // Shared text-field alert (same component as route rename).
        .obcRenameAlert(
            "Rename device",
            isPresented: $renameShown,
            name: $renameDraft,
            message: "Shown across the app and on the device.",
            onSave: { _ = model.rename(to: renameDraft) }
        )
        .task { model.start() }
    }

    // MARK: Device (G + H2/H3)

    private var deviceGroup: some View {
        OBCGroupedSection(
            "Device",
            footer: "Forgetting removes the bond. Your routes and rides stay on this phone."
        ) {
            deviceRow
            OBCListRow(
                icon: "pencil",
                iconColor: OBCTheme.water,
                label: "Rename device",
                showsChevron: true,
                disabled: !model.canRename,
                action: {
                    renameDraft = model.deviceName
                    renameShown = true
                }
            )
            OBCListRow(
                icon: "power",
                iconColor: OBCTheme.warning,
                label: "Forget device",
                labelColor: OBCTheme.warning,
                showsDivider: false,
                action: { forgetShown = true }
            )
            // Hangs off the row, not the scroll root — confirmationDialog
            // anchors to the attached view on iOS 26.
            .obcDestructiveConfirm(
                "Forget \(model.deviceName)?",
                isPresented: $forgetShown,
                message: "You'll pair again to use it. Your routes and rides stay on this phone.",
                actionTitle: "Forget device",
                onConfirm: { model.forget() }
            )
        }
    }

    /// The identity row: name over the live status line, firmware trailing.
    private var deviceRow: some View {
        HStack(spacing: 12) {
            OBCIconTile(systemImage: "flipphone", color: OBCTheme.forest)
            VStack(alignment: .leading, spacing: 2) {
                Text(model.deviceName)
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(OBCTheme.ink)
                Text(model.statusLine)
                    .font(.obcMono(size: 12))
                    .foregroundStyle(model.isConnected ? OBCTheme.forest : OBCTheme.inkFaint)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if let firmware = model.firmwareDisplay {
                Text(firmware)
                    .font(.system(size: 15))
                    .foregroundStyle(OBCTheme.inkFaint)
            }
        }
        .padding(.vertical, 14)
        .padding(.horizontal, 16)
        .frame(minHeight: 52)
        .overlay(alignment: .bottom) {
            OBCTheme.screenLine.frame(height: 1).padding(.leading, 56)
        }
    }

    // MARK: Firmware (coming soon)

    private var firmwareGroup: some View {
        OBCGroupedSection(
            "Firmware",
            footer: "OTA updates will arrive in a later release. For now, flash from the desktop tool."
        ) {
            OBCListRow(
                icon: "arrow.down.to.line",
                iconColor: OBCTheme.amber,
                label: "Update over the air",
                comingSoon: true
            )
            OBCListRow(
                icon: "clock",
                iconColor: OBCTheme.parchment3,
                label: "Firmware version",
                value: model.firmwareLine,
                showsDivider: false
            )
        }
    }

    // MARK: Connected services (the B7 seam, disabled)

    private var servicesGroup: some View {
        OBCGroupedSection(
            "Connected services",
            footer: "Later: link a service, then flip auto-sync on import to push every new "
                + "ride automatically. Off or a push fails? Upload a single ride from its "
                + "detail — your choice, on your device."
        ) {
            OBCListRow(
                icon: "bolt.fill",
                iconColor: OBCTheme.coral,
                label: "Strava sync",
                comingSoon: true
            )
            OBCListRow(
                icon: "map",
                iconColor: OBCTheme.wood,
                label: "Komoot sync",
                comingSoon: true
            )
            OBCListRow(
                icon: "square.and.arrow.down",
                iconColor: OBCTheme.amber,
                label: "Auto-sync on import",
                disabled: true,
                showsDivider: false
            ) {
                // Present but non-functional by design.
                Toggle("Auto-sync on import", isOn: .constant(false))
                    .labelsHidden()
                    .disabled(true)
                    .tint(OBCTheme.forest)
                OBCSoonBadge("Soon")
            }
        }
    }

    // MARK: About

    private var aboutGroup: some View {
        OBCGroupedSection(
            "About",
            footer: "No account. No subscription. No cloud."
        ) {
            OBCListRow(
                icon: "chevron.left.forwardslash.chevron.right",
                iconColor: OBCTheme.ink,
                label: "OpenBikeComputer on GitHub",
                showsChevron: true,
                action: { openURL(Self.gitHubURL) }
            )
            OBCListRow(
                icon: "info.circle",
                iconColor: OBCTheme.parchment3,
                label: "App version",
                value: Self.appVersion,
                showsDivider: false,
                // Hidden dev-panel entry (Debug wiring only): five taps.
                action: onOpenDevPanel == nil ? nil : {
                    versionTaps += 1
                    if versionTaps >= 5 {
                        versionTaps = 0
                        onOpenDevPanel?()
                    }
                }
            )
        }
    }

    /// "1.0 (build 12)" from the app bundle — the package previews show the
    /// preview host's numbers, which is fine.
    private static var appVersion: String {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString")
            as? String ?? "1.0"
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "1"
        return "\(version) (build \(build))"
    }
}

#if DEBUG
#Preview("Settings (G)") {
    // Preview-only placeholder wiring (OBCUI can't import OBCMock — see the
    // app target's RootView previews for the transport-driven screens).
    NavigationStack {
        SettingsView(model: SettingsModel(
            transport: PreviewSettingsTransport(),
            bondStore: PreviewNoopBondStore()
        ))
    }
}

/// Inert transport for `#Preview` construction only — serves a fixed identity
/// (Trailhead · 82% · v0.4.2) and nothing else.
private struct PreviewSettingsTransport: DeviceTransport {
    var state: AsyncStream<ConnectionState> {
        AsyncStream { $0.yield(.connected); $0.finish() }
    }
    var battery: AsyncStream<Int> { AsyncStream { $0.yield(82); $0.finish() } }
    func connect() async throws {}
    func disconnect() async {}
    func deviceInfo() async throws -> DeviceInfo {
        DeviceInfo(name: "Trailhead", firmwareVersion: "0.4.2")
    }
    func readConfig() async throws -> DeviceConfig { DeviceConfig(name: "Trailhead") }
    func writeConfig(_ config: DeviceConfig) async throws {}
    func listRoutes() async throws -> [RouteSummary] { [] }
    func routeDetail(_ id: RouteID) async throws -> RouteDetail { throw DeviceError.readFailed }
    func uploadRoute(_ route: RouteBlob) -> TransferHandle {
        .immediatelyFinished(.failed(.notConnected))
    }
    func deleteRoute(_ id: RouteID) async throws {}
    func listRides() async throws -> [RideSummary] { [] }
    func rideDetail(_ id: RideID) async throws -> RideDetail { throw DeviceError.readFailed }
    func downloadRides(_ ids: [RideID]) -> RideDownload { .finished() }
    func readDiagnostics() async throws -> Data { Data() }
}

private struct PreviewNoopBondStore: BondStore {
    func load() -> BondRecord? { BondRecord(deviceName: "Trailhead") }
    func save(_ record: BondRecord) {}
    func clear() {}
}
#endif
