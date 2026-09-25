import SwiftUI
import UniformTypeIdentifiers
import OBCDomain
import OBCTransport

/// The firmware-update screen: the running version, the published release, the staged
/// file, and one action to send it. The rider confirms on the device, which restarts;
/// the screen follows that through to the reconnect. A release is proved against the
/// manifest size and SHA-256 before it stages; the Files picker is the only path for a
/// device whose running version cannot be parsed. A corrupt file never leaves the phone.
public struct FirmwareUpdateView: View {
    @Bindable private var model: FirmwareUpdateModel
    @State private var pickerShown = false
    @Environment(\.openURL) private var openURL

    public init(model: FirmwareUpdateModel) {
        self.model = model
    }

    /// Resolved by extension so the app declares no imported type. `.data` covers a
    /// `.bin` that iOS types as generic.
    private var contentTypes: [UTType] {
        [UTType(filenameExtension: "bin") ?? .data, .data]
    }

    public var body: some View {
        ScrollView {
            VStack(spacing: 26) {
                runningGroup
                availableGroup
                stagedGroup
                #if DEBUG
                developerGroup
                #endif
            }
            .padding(.horizontal, 20)
            .padding(.top, 18)
            .padding(.bottom, 30)
        }
        .background(OBCTheme.page.ignoresSafeArea())
        .navigationTitle("Firmware update")
        #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
        #endif
        .accessibilityIdentifier("firmware.screen")
        .fileImporter(
            isPresented: $pickerShown,
            allowedContentTypes: contentTypes
        ) { result in
            if case .success(let url) = result { model.stageFile(at: url) }
        }
        // A picked file that isn't a usable update dies here, not on the device.
        .alert(
            "Can't use that file",
            isPresented: Binding(
                get: { model.importError != nil },
                set: { if !$0 { model.importError = nil } }
            )
        ) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(model.importError ?? "")
        }
        .task { model.start() }
        // Popping mid-send must not leave a headless transfer or a leaked ledger claim.
        // The pair is re-entrant: if SwiftUI cycles disappear/appear on a persisting
        // model, `.task` runs `start()` again and re-subscribes the link state.
        .onDisappear { model.stop() }
    }

    // MARK: Running version + the check

    private var runningGroup: some View {
        OBCGroupedSection("On the device", footer: statusFooter) {
            OBCListRow(
                icon: "cpu",
                iconColor: OBCTheme.tint,
                label: "Firmware version",
                value: model.connection == .connected ? model.runningVersionLine : "—",
                showsDivider: model.supportsUpdateCheck
            )
            if model.supportsUpdateCheck {
                OBCListRow(
                    icon: "arrow.clockwise",
                    iconColor: OBCTheme.tint,
                    label: "Check for updates",
                    value: model.checkState == .checking ? nil : model.lastCheckedLine,
                    showsDivider: false,
                    action: { model.checkForUpdate(manual: true) }
                ) {
                    if model.checkState == .checking {
                        ProgressView().controlSize(.small)
                    }
                }
                .accessibilityIdentifier("firmware.checkForUpdates")
            }
        }
    }

    /// The quiet answers go in the section footer: up to date, ahead of the published
    /// build, or a development build. `available` gets its own section.
    private var statusFooter: String? {
        guard model.supportsUpdateCheck, model.hasUpdateAnswer else { return nil }
        switch model.updateStatus {
        case .available, .noRelease:
            return nil
        case .current:
            return "Up to date."
        case .unknown:
            return "Development build — automatic updates are paused."
        case .ahead:
            return "\(model.deviceName) is newer than the published \(model.latestVersionLine)."
        }
    }

    // MARK: Available update

    /// A newer published build and the one action. Failure lines live here too: a
    /// download that fails verification staged nothing, so there is nowhere else.
    @ViewBuilder
    private var availableGroup: some View {
        if case .failed(let message) = model.checkState {
            noticeCard(icon: "exclamationmark.triangle", tint: OBCTheme.danger, text: message)
        }
        if model.updateStatus == .available {
            VStack(spacing: 16) {
                // Once a file is staged, its own section carries the safety line.
                OBCGroupedSection("Update available", footer: model.phase == .idle ? model.safetyLine : nil) {
                    releaseRow
                    if let notes = model.releaseNotesURL {
                        OBCListRow(
                            icon: "doc.text",
                            iconColor: OBCTheme.tint,
                            label: "Release notes",
                            showsChevron: true,
                            showsDivider: false,
                            action: { openURL(notes) }
                        )
                        .accessibilityIdentifier("firmware.releaseNotes")
                    }
                }

                if case .failed(let message) = model.downloadState {
                    noticeCard(icon: "exclamationmark.triangle", tint: OBCTheme.danger, text: message)
                }

                if model.downloadState == .downloading {
                    HStack(spacing: 10) {
                        ProgressView().controlSize(.small)
                        Text("Downloading update…")
                            .font(.system(.footnote))
                            .foregroundStyle(OBCTheme.secondary)
                    }
                    .frame(maxWidth: .infinity, alignment: .center)
                } else {
                    Button("Download & Install") { model.downloadUpdate() }
                        .buttonStyle(.obcPrimary)
                        .disabled(!model.canDownloadUpdate)
                        .accessibilityIdentifier("firmware.downloadAndInstall")
                }
            }
        }
    }

    private var releaseRow: some View {
        HStack(spacing: 12) {
            OBCIconTile(systemImage: "arrow.down.to.line", color: OBCTheme.tint)
            VStack(alignment: .leading, spacing: 2) {
                Text(model.latestVersionLine)
                    .font(.system(.callout, weight: .semibold))
                    .foregroundStyle(OBCTheme.ink)
                Text(model.latestSizeLine)
                    .font(.system(.caption).monospacedDigit())
                    .foregroundStyle(OBCTheme.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 14)
        .padding(.horizontal, 16)
        .frame(minHeight: 52)
        .overlay(alignment: .bottom) {
            if model.releaseNotesURL != nil {
                OBCTheme.hairline.frame(height: 1).padding(.leading, 56)
            }
        }
        .accessibilityIdentifier("firmware.availableUpdate")
    }

    // MARK: Staged file + action

    @ViewBuilder
    private var stagedGroup: some View {
        switch model.phase {
        case .idle:
            idleGroup
        case .staged, .failed:
            stagedFileGroup
        case .transferring, .interrupted:
            transferGroup
        case .awaitingConfirm:
            awaitingGroup
        case .done:
            doneGroup
        }
    }

    private var idleGroup: some View {
        OBCGroupedSection(
            "Update file",
            footer: "Import the UPDATE.BIN you downloaded for OpenBikeComputer. It's checked before anything is sent."
        ) {
            OBCListRow(
                icon: "square.and.arrow.down",
                iconColor: OBCTheme.tint,
                label: "Choose update file",
                showsChevron: true,
                showsDivider: false,
                action: { pickerShown = true }
            )
        }
    }

    /// A validated file, Send, and a way to swap it. `.failed` reuses this group with
    /// its failure line above the button.
    private var stagedFileGroup: some View {
        VStack(spacing: 16) {
            OBCGroupedSection("Update file", footer: model.safetyLine) {
                stagedFileRow
                OBCListRow(
                    icon: "arrow.triangle.2.circlepath",
                    iconColor: OBCTheme.tint,
                    label: "Choose a different file",
                    showsChevron: true,
                    showsDivider: false,
                    action: { pickerShown = true }
                )
            }

            if let failure = model.failureMessage {
                noticeCard(icon: "exclamationmark.triangle", tint: OBCTheme.danger, text: failure)
            } else if model.stagedMatchesRunning {
                noticeCard(
                    icon: "checkmark",
                    tint: OBCTheme.secondary,
                    text: "\(model.deviceName) is already running this version."
                )
            }

            Button("Send to \(model.deviceName)") { model.send() }
                .buttonStyle(.obcPrimary)
                .disabled(!model.canSend)
                .accessibilityIdentifier("firmware.send")

            if model.connection != .connected {
                Text("Connect to \(model.deviceName) to send the update.")
                    .font(.system(.footnote))
                    .foregroundStyle(OBCTheme.secondary)
                    .frame(maxWidth: .infinity, alignment: .center)
            }
        }
    }

    private var stagedFileRow: some View {
        HStack(spacing: 12) {
            OBCIconTile(systemImage: "doc", color: OBCTheme.tint)
            VStack(alignment: .leading, spacing: 2) {
                Text(model.stagedVersionLine)
                    .font(.system(.callout, weight: .semibold))
                    .foregroundStyle(OBCTheme.ink)
                Text(model.stagedSizeLine)
                    .font(.system(.caption).monospacedDigit())
                    .foregroundStyle(OBCTheme.secondary)
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

    /// Streaming to the device: progress and cancel. `.interrupted` swaps in Resume.
    private var transferGroup: some View {
        VStack(spacing: 16) {
            OBCGroupedSection("Sending to \(model.deviceName)") {
                VStack(alignment: .leading, spacing: 10) {
                    HStack {
                        Text(model.stagedVersionLine)
                            .font(.system(.subheadline, weight: .semibold))
                            .foregroundStyle(OBCTheme.ink)
                        Spacer()
                        Text(model.percentLine)
                            .font(.obcStat(.footnote))
                            .foregroundStyle(OBCTheme.ink)
                    }
                    OBCProgressBar(value: model.fraction)
                    if model.phase == .interrupted {
                        Text("The link dropped. Nothing was installed. Resume sends it again from the start.")
                            .font(.system(.footnote))
                            .foregroundStyle(OBCTheme.secondary)
                    }
                }
                .padding(16)
                .accessibilityElement(children: .combine)
            }

            if model.phase == .interrupted {
                Button("Resume") { model.resume() }
                    .buttonStyle(.obcPrimary)
            }
            Button("Cancel") { model.cancel() }
                .buttonStyle(.obcGhost)
        }
    }

    /// installFw accepted: the device shows its confirm card, then the frame it holds while it
    /// installs.
    private var awaitingGroup: some View {
        deviceState(
            model.isInstalling
                ? .installing
                : .confirm(installed: model.runningVersion ?? "", update: model.staged?.version ?? ""),
            title: model.awaitingTitle,
            message: model.awaitingMessage
        )
    }

    /// The device reconnected on the staged version.
    private var doneGroup: some View {
        deviceState(
            .updated(version: model.runningVersion ?? ""),
            title: "Update complete",
            message: model.doneMessage
        )
    }

    private func deviceState(_ card: DeviceFirmwareCard, title: String, message: String) -> some View {
        VStack(spacing: 0) {
            DeviceGlyphView(variant: .firmware(card))
                .padding(.top, 8)
                .padding(.bottom, 28)
            Text(title)
                .font(.system(.title2, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .multilineTextAlignment(.center)
                .accessibilityAddTraits(.isHeader)
                .accessibilityIdentifier("firmware.stateTitle")
            Text(message)
                .font(.system(.body))
                .foregroundStyle(OBCTheme.secondary)
                .multilineTextAlignment(.center)
                .padding(.top, 6)
        }
        .frame(maxWidth: .infinity)
    }

    #if DEBUG
    /// Debug-only pre-release opt-in, never part of the shipped screen. On, the check
    /// also reads the pre-release manifest and offers the newer channel.
    @ViewBuilder
    private var developerGroup: some View {
        if model.supportsUpdateCheck {
            OBCGroupedSection(
                "Developer",
                footer: "Pre-release builds are unfinished by definition. Leave this off unless "
                    + "you're testing one."
            ) {
                OBCListRow(
                    icon: "hammer",
                    iconColor: OBCTheme.tint,
                    label: "Include pre-releases",
                    showsDivider: false
                ) {
                    Toggle(
                        "Include pre-releases",
                        isOn: Binding(
                            get: { model.includePrereleases },
                            set: { model.setIncludePrereleases($0) }
                        )
                    )
                    .labelsHidden()
                    .tint(OBCTheme.tint)
                }
                .accessibilityIdentifier("firmware.includePrereleases")
            }
        }
    }
    #endif

    private func noticeCard(icon: String, tint: Color, text: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon)
                .font(.system(.subheadline, weight: .semibold))
                .foregroundStyle(tint)
            Text(text)
                .font(.system(.footnote))
                .foregroundStyle(OBCTheme.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(14)
        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
    }
}

#if DEBUG
#Preview("Firmware update — staged") {
    NavigationStack {
        FirmwareUpdateView(model: {
            let model = FirmwareUpdateModel(transport: PreviewFirmwareTransport(), deviceName: "Trailhead")
            model.stage(PreviewFirmwareTransport.sampleContainer)
            return model
        }())
    }
}

private struct PreviewFirmwareTransport: DeviceLink, DeviceUpdates {
    var state: AsyncStream<ConnectionState> {
        AsyncStream { $0.yield(.connected); $0.finish() }
    }
    func connect() async throws {}
    func disconnect() async {}
    func deviceInfo() async throws -> DeviceInfo { DeviceInfo(name: "Trailhead", firmwareVersion: "0.4.2") }

    /// A minimal valid OBCU container: 64-byte header and a tiny raw image, both CRCs
    /// correct.
    static let sampleContainer: Data = {
        var image = Data([0x00, 0x00, 0x02, 0x20])  // plausible initial SP, LE
        image.append(contentsOf: (4..<64).map { UInt8($0 & 0xFF) })
        var header = Data(count: 64)
        header.replaceSubrange(0..<4, with: Array("OBCU".utf8))
        header[4] = 1  // header version LE
        withUnsafeBytes(of: UInt32(image.count).littleEndian) { header.replaceSubrange(8..<12, with: $0) }
        withUnsafeBytes(of: CRC32.checksum(image).littleEndian) { header.replaceSubrange(12..<16, with: $0) }
        header.replaceSubrange(16..<16 + 6, with: Array("0.5.0".utf8))
        let hcrc = CRC32.checksum(header[0..<60])
        withUnsafeBytes(of: hcrc.littleEndian) { header.replaceSubrange(60..<64, with: $0) }
        return header + image
    }()
}
#endif
