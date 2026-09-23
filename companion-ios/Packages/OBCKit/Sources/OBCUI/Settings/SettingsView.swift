import SwiftUI
import OBCDomain
import OBCTransport

/// The Settings screen: device management, a firmware section, the connected-services
/// seam, and About. Nothing here implies a cloud or an account.
public struct SettingsView: View {
    @Bindable private var model: SettingsModel
    /// Push the firmware-update screen. `nil` keeps the Firmware row a coming-soon
    /// placeholder (previews, and any wiring that does not host the update screen).
    private let onOpenFirmwareUpdate: (() -> Void)?

    /// Debug-only: five taps on the App version row open the mock dev panel. `nil` in
    /// Release wiring, where the gesture goes nowhere.
    private let onOpenDevPanel: (() -> Void)?

    @State private var renameShown = false
    @State private var forgetShown = false
    @State private var versionTaps = 0
    @Environment(\.openURL) private var openURL

    private static let gitHubURL = URL(string: "https://github.com/timohueser/OpenBikeComputer")!

    public init(
        model: SettingsModel,
        onOpenFirmwareUpdate: (() -> Void)? = nil,
        onOpenDevPanel: (() -> Void)? = nil
    ) {
        self.model = model
        self.onOpenFirmwareUpdate = onOpenFirmwareUpdate

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
        .obcRenameSheet(
            "Rename device",
            isPresented: $renameShown,
            name: model.deviceName,
            message: "Shown across the app and on the device.",
            onSave: { _ = model.rename(to: $0) }
        )
        // The rename's config write failed: say so once. The reconcile pass pushes the
        // name on the next connect, so no action is needed.
        .obcToast(
            isPresented: $model.renameWriteFailed,
            systemImage: "exclamationmark.triangle",
            message: "Couldn't update the name on \(model.deviceName). "
                + "It'll retry next time you connect.",
            duration: .seconds(4)
        )
        .task { model.start() }
    }

    // MARK: Device

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
            // Hangs off the row, not the scroll root: confirmationDialog anchors to
            // the attached view on iOS 26.
            .obcDestructiveConfirm(
                "Forget \(model.deviceName)?",
                isPresented: $forgetShown,
                message: model.forgetMessage,
                actionTitle: "Forget device",
                onConfirm: { model.forget() }
            )
        }
    }

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
    // MARK: Firmware

    private var firmwareGroup: some View {
        OBCGroupedSection("Firmware", footer: firmwareFooter) {
            if let onOpenFirmwareUpdate {
                OBCListRow(
                    icon: "arrow.down.to.line",
                    iconColor: OBCTheme.amber,
                    label: "Update firmware",
                    showsChevron: true,
                    action: onOpenFirmwareUpdate
                )
            } else {
                OBCListRow(
                    icon: "arrow.down.to.line",
                    iconColor: OBCTheme.amber,
                    label: "Update over the air",
                    comingSoon: true
                )
            }
            // The one switch behind the launch sheet and the background check.
            OBCListRow(
                icon: "arrow.clockwise",
                iconColor: OBCTheme.water,
                label: "Check for updates automatically"
            ) {
                Toggle(
                    "Check for updates automatically",
                    isOn: Binding(
                        get: { model.autoCheckUpdates },
                        set: { model.setAutoCheckUpdates($0) }
                    )
                )
                .labelsHidden()
                .tint(OBCTheme.forest)
            }
            .accessibilityIdentifier("firmware.autoCheck")
            OBCListRow(
                icon: "clock",
                iconColor: OBCTheme.parchment3,
                label: "Firmware version",
                value: model.firmwareLine,
                showsDivider: false
            )
        }
    }

    private var firmwareFooter: String {
        guard onOpenFirmwareUpdate != nil else {
            return "OTA updates will arrive in a later release. For now, flash from the desktop tool."
        }
        return "Send new firmware over Bluetooth — a file you picked, or the published update the "
            + "app finds for you. Checking is an anonymous request for one public file: no account, "
            + "and nothing about your device or your rides is sent."
    }

    // MARK: Connected services

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

    /// "1.0 (build 12)" from the app bundle.
    private static var appVersion: String {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString")
            as? String ?? "1.0"
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "1"
        return "\(version) (build \(build))"
    }
}

#if DEBUG
#Preview("Settings (G)") {
    // OBCUI cannot import OBCMock, so previews use placeholder wiring.
    NavigationStack {
        SettingsView(model: SettingsModel(
            transport: PreviewSettingsTransport(),
            bondStore: PreviewNoopBondStore()
        ))
    }
}

private struct PreviewSettingsTransport: DeviceLink, DeviceBattery, DeviceConfiguration, DeviceBonding {
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
}

private struct PreviewNoopBondStore: BondStore {
    func load() -> BondRecord? { BondRecord(deviceName: "Trailhead") }
    func save(_ record: BondRecord) {}
    func clear() {}
}
#endif
