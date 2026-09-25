import SwiftUI
import OBCDomain
import OBCTransport

/// The Settings screen: device management, firmware, the app's appearance, and About. Nothing
/// here implies a cloud or an account.
public struct SettingsView: View {
    @Bindable private var model: SettingsModel
    /// Push the firmware-update screen. `nil` leaves the Update firmware row out (previews, and
    /// any wiring that does not host the update screen).
    private let onOpenFirmwareUpdate: (() -> Void)?

    /// Debug-only: five taps on the App version row open the mock dev panel. `nil` in
    /// Release wiring, where the gesture goes nowhere.
    private let onOpenDevPanel: (() -> Void)?

    @State private var renameShown = false
    @State private var forgetShown = false
    @State private var versionTaps = 0
    @Environment(\.openURL) private var openURL

    private static let gitHubURL = URL(string: "https://github.com/timohueser/OpenBikeComputer")!
    private static let docsURL = URL(string: "https://openbikecomputer.com/docs/")!

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
                appGroup
                aboutGroup
            }
            .padding(.horizontal, 20)
            .padding(.top, 18)
            .padding(.bottom, 30)
        }
        .background(OBCTheme.page.ignoresSafeArea())
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
        OBCGroupedSection("Device") {
            deviceRow
            OBCListRow(
                icon: "pencil",
                iconColor: OBCTheme.tint,
                label: "Rename device",
                showsChevron: true,
                disabled: !model.canRename,
                action: {
                    renameShown = true
                }
            )
            OBCListRow(
                icon: "xmark.circle",
                iconColor: OBCTheme.danger,
                label: "Forget device",
                labelColor: OBCTheme.danger,
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
            OBCIconTile(systemImage: "flipphone", color: OBCTheme.tint)
            VStack(alignment: .leading, spacing: 2) {
                Text(model.deviceName)
                    .font(.system(.callout, weight: .semibold))
                    .foregroundStyle(OBCTheme.ink)
                Text(model.statusLine)
                    .font(.system(.caption).monospacedDigit())
                    .foregroundStyle(model.isConnected ? OBCTheme.ink : OBCTheme.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 14)
        .padding(.horizontal, 16)
        .frame(minHeight: 52)
        .overlay(alignment: .bottom) {
            OBCTheme.hairline.frame(height: 1).padding(.leading, 56)
        }
    }
    // MARK: Firmware

    private var firmwareGroup: some View {
        OBCGroupedSection("Firmware", footer: "Updates install only after you confirm on \(model.deviceName).") {
            if let onOpenFirmwareUpdate {
                OBCListRow(
                    icon: "arrow.down.to.line",
                    iconColor: OBCTheme.tint,
                    label: "Update firmware",
                    showsChevron: true,
                    action: onOpenFirmwareUpdate
                )
            }
            // The one switch behind the launch sheet and the background check.
            OBCListRow(
                icon: "arrow.clockwise",
                iconColor: OBCTheme.tint,
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
                .tint(OBCTheme.tint)
            }
            .accessibilityIdentifier("firmware.autoCheck")
            OBCListRow(
                icon: "cpu",
                iconColor: OBCTheme.tint,
                label: "Firmware version",
                value: model.firmwareLine,
                showsDivider: false
            )
        }
    }

    // MARK: App

    private var appGroup: some View {
        OBCGroupedSection("App", footer: "Automatic follows the setting on your iPhone.") {
            OBCAppearanceRow()
        }
    }

    // MARK: About

    private var aboutGroup: some View {
        OBCGroupedSection(
            "About",
            footer: "No account. No subscription. No cloud."
        ) {
            OBCListRow(
                icon: "book",
                iconColor: OBCTheme.tint,
                label: "Documentation",
                showsChevron: true,
                action: { openURL(Self.docsURL) }
            )
            OBCListRow(
                icon: "chevron.left.forwardslash.chevron.right",
                iconColor: OBCTheme.tint,
                label: "OpenBikeComputer on GitHub",
                showsChevron: true,
                action: { openURL(Self.gitHubURL) }
            )
            OBCListRow(
                icon: "info.circle",
                iconColor: OBCTheme.tint,
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
