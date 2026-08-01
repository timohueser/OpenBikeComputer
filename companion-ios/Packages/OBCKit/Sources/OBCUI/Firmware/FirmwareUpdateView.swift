import SwiftUI
import UniformTypeIdentifiers
import OBCDomain
import OBCTransport

/// The firmware-update screen (S7) — pushed from Settings' Firmware group. Shows
/// the running version, what's published, the staged file (version + size), and a
/// single action: send it. After sending, the rider confirms on the device and it
/// restarts; the screen tracks that through to the reconnect.
///
/// Two ways in, one way out. The **published release** (#773 U4) is checked on
/// appear — an anonymous GET for a public manifest, nothing about the device sent —
/// and a newer build offers "Download & Install": the container is downloaded and
/// proved against the manifest's size + SHA-256, then handed to the *same* staging
/// path the picker feeds. The **Files picker** (`UPDATE.BIN` / `.bin`) stays, and
/// is the only path for a device whose running version can't be parsed. Either
/// way a corrupt file is refused on the phone, never streamed to the device, and
/// nothing installs until the rider confirms it on the glass.
public struct FirmwareUpdateView: View {
    @Bindable private var model: FirmwareUpdateModel
    @State private var pickerShown = false
    @Environment(\.openURL) private var openURL

    public init(model: FirmwareUpdateModel) {
        self.model = model
    }

    /// `UPDATE.BIN` and any `.bin` — resolved by extension so the app declares no
    /// imported type. `.data` is the fallback for a `.bin` iOS types as generic.
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
        .background(OBCTheme.parchment.ignoresSafeArea())
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
        // Popping the screen mid-send must not leave a headless transfer (or a
        // leaked #459/#754 ledger claim keeping the screen awake) behind it.
        // The pair is re-entrant: if SwiftUI ever cycles disappear/appear on a
        // persisting model (a presentation pushed over S7, a scene re-attach),
        // `.task` runs `start()` again and it re-subscribes the link state.
        .onDisappear { model.stop() }
    }

    // MARK: Running version + the check

    private var runningGroup: some View {
        OBCGroupedSection("On the device", footer: statusFooter) {
            OBCListRow(
                icon: "cpu",
                iconColor: OBCTheme.forest,
                label: "Firmware version",
                value: model.connection == .connected ? model.runningVersionLine : "—",
                showsDivider: model.supportsUpdateCheck
            )
            if model.supportsUpdateCheck {
                OBCListRow(
                    icon: "arrow.clockwise",
                    iconColor: OBCTheme.water,
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

    /// The quiet answers live in the section footer — up to date, ahead of the
    /// published build, or a development build that isn't offered updates at all.
    /// `available` gets its own section instead, and `noRelease` says nothing.
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

    // MARK: Available update (#773 U4)

    /// A newer published build: what it is, where to read about it, and the one
    /// action. The failure lines for a check or a download the rider asked for
    /// live here too — a download that fails its own verification never staged
    /// anything, so this is the only place it can be said.
    @ViewBuilder
    private var availableGroup: some View {
        if case .failed(let message) = model.checkState {
            noticeCard(icon: "exclamationmark.triangle", tint: OBCTheme.warning, text: message)
        }
        if model.updateStatus == .available {
            VStack(spacing: 16) {
                OBCGroupedSection(
                    "Update available",
                    footer: "Downloaded and checked on this phone, then sent over Bluetooth. "
                        + "Nothing is installed until you confirm it on \(model.deviceName)."
                ) {
                    releaseRow
                    if let notes = model.releaseNotesURL {
                        OBCListRow(
                            icon: "doc.text",
                            iconColor: OBCTheme.wood,
                            label: "Release notes",
                            showsChevron: true,
                            showsDivider: false,
                            action: { openURL(notes) }
                        )
                        .accessibilityIdentifier("firmware.releaseNotes")
                    }
                }

                if case .failed(let message) = model.downloadState {
                    noticeCard(icon: "exclamationmark.triangle", tint: OBCTheme.warning, text: message)
                }

                if model.downloadState == .downloading {
                    HStack(spacing: 10) {
                        ProgressView().controlSize(.small)
                        Text("Downloading update…")
                            .font(.system(size: 13.5))
                            .foregroundStyle(OBCTheme.inkSoft)
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

    /// The published build: version over size, matching the staged-file row.
    private var releaseRow: some View {
        HStack(spacing: 12) {
            OBCIconTile(systemImage: "sparkles", color: OBCTheme.amber)
            VStack(alignment: .leading, spacing: 2) {
                Text(model.latestVersionLine)
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(OBCTheme.ink)
                Text(model.latestSizeLine)
                    .font(.obcMono(size: 12))
                    .foregroundStyle(OBCTheme.inkFaint)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 14)
        .padding(.horizontal, 16)
        .frame(minHeight: 52)
        .overlay(alignment: .bottom) {
            if model.releaseNotesURL != nil {
                OBCTheme.screenLine.frame(height: 1).padding(.leading, 56)
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

    /// No file yet — the one action is to choose one.
    private var idleGroup: some View {
        OBCGroupedSection(
            "Update file",
            footer: "Import the UPDATE.BIN you downloaded for OpenBikeComputer. It's checked before anything is sent."
        ) {
            OBCListRow(
                icon: "square.and.arrow.down",
                iconColor: OBCTheme.water,
                label: "Choose update file",
                showsChevron: true,
                showsDivider: false,
                action: { pickerShown = true }
            )
        }
    }

    /// A validated file: version + size, then Send (dimmed off-link) and a way to
    /// swap the file. `.failed` reuses this with its failure line above the button.
    private var stagedFileGroup: some View {
        VStack(spacing: 16) {
            OBCGroupedSection("Update file") {
                stagedFileRow
                OBCListRow(
                    icon: "arrow.triangle.2.circlepath",
                    iconColor: OBCTheme.wood,
                    label: "Choose a different file",
                    showsChevron: true,
                    showsDivider: false,
                    action: { pickerShown = true }
                )
            }

            if let failure = model.failureMessage {
                noticeCard(icon: "exclamationmark.triangle", tint: OBCTheme.warning, text: failure)
            } else if model.stagedMatchesRunning {
                noticeCard(
                    icon: "checkmark.seal",
                    tint: OBCTheme.forest,
                    text: "\(model.deviceName) is already running this version."
                )
            }

            Button("Send to bike computer") { model.send() }
                .buttonStyle(.obcPrimary)
                .disabled(!model.canSend)

            if model.connection != .connected {
                Text("Connect to \(model.deviceName) to send the update.")
                    .font(.system(size: 12.5))
                    .foregroundStyle(OBCTheme.inkFaint)
                    .frame(maxWidth: .infinity, alignment: .center)
            }
        }
    }

    private var stagedFileRow: some View {
        HStack(spacing: 12) {
            OBCIconTile(systemImage: "shippingbox", color: OBCTheme.amber)
            VStack(alignment: .leading, spacing: 2) {
                Text(model.stagedVersionLine)
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(OBCTheme.ink)
                Text(model.stagedSizeLine)
                    .font(.obcMono(size: 12))
                    .foregroundStyle(OBCTheme.inkFaint)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 14)
        .padding(.horizontal, 16)
        .frame(minHeight: 52)
        .overlay(alignment: .bottom) {
            OBCTheme.screenLine.frame(height: 1).padding(.leading, 56)
        }
    }

    /// Streaming to the device — progress + cancel. `.interrupted` swaps in Resume.
    private var transferGroup: some View {
        VStack(spacing: 16) {
            OBCGroupedSection("Sending update") {
                VStack(alignment: .leading, spacing: 10) {
                    HStack {
                        Text(model.stagedVersionLine)
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundStyle(OBCTheme.ink)
                        Spacer()
                        Text(model.percentLine)
                            .font(.obcMono(size: 13))
                            .foregroundStyle(OBCTheme.forest)
                    }
                    OBCProgressBar(value: model.fraction)
                    if model.phase == .interrupted {
                        Text("The link dropped. Resume sends it again from the start.")
                            .font(.system(size: 12.5))
                            .foregroundStyle(OBCTheme.inkFaint)
                    }
                }
                .padding(16)
            }

            if model.phase == .interrupted {
                Button("Resume") { model.resume() }
                    .buttonStyle(.obcPrimary)
            }
            Button("Cancel") { model.cancel() }
                .buttonStyle(.obcGhost)
        }
    }

    /// installFw accepted — the rider confirms on the device, which reboots.
    private var awaitingGroup: some View {
        OBCGroupedSection {
            VStack(spacing: 12) {
                Image(systemName: "arrow.triangle.2.circlepath")
                    .font(.system(size: 26, weight: .regular))
                    .foregroundStyle(OBCTheme.forest)
                Text(model.awaitingTitle)
                    .font(.obcSerif(size: 20))
                    .foregroundStyle(OBCTheme.ink)
                Text(model.awaitingMessage)
                    .font(.system(size: 14))
                    .foregroundStyle(OBCTheme.inkSoft)
                    .multilineTextAlignment(.center)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 26)
            .padding(.horizontal, 18)
        }
    }

    /// The device reconnected on the staged version.
    private var doneGroup: some View {
        OBCGroupedSection {
            VStack(spacing: 12) {
                Image(systemName: "checkmark.seal.fill")
                    .font(.system(size: 30, weight: .regular))
                    .foregroundStyle(OBCTheme.forest)
                Text("Update complete")
                    .font(.obcSerif(size: 20))
                    .foregroundStyle(OBCTheme.ink)
                Text(model.doneMessage)
                    .font(.system(size: 14))
                    .foregroundStyle(OBCTheme.inkSoft)
                    .multilineTextAlignment(.center)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 26)
            .padding(.horizontal, 18)
        }
    }

    #if DEBUG
    /// The pre-release channel opt-in — a Debug-only developer switch, never part
    /// of the shipped screen. On, the check also reads the pre-release manifest and
    /// offers whichever channel is newer.
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
                    iconColor: OBCTheme.parchment3,
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
                    .tint(OBCTheme.forest)
                }
                .accessibilityIdentifier("firmware.includePrereleases")
            }
        }
    }
    #endif

    private func noticeCard(icon: String, tint: Color, text: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon)
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(tint)
            Text(text)
                .font(.system(size: 13.5))
                .foregroundStyle(OBCTheme.inkSoft)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(14)
        .background(OBCTheme.panel)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
        .overlay(
            RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.line)
        )
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

/// Inert transport for the preview — a connected device on v0.4.2.
private struct PreviewFirmwareTransport: DeviceTransport {
    var state: AsyncStream<ConnectionState> {
        AsyncStream { $0.yield(.connected); $0.finish() }
    }
    var battery: AsyncStream<Int> { AsyncStream { $0.finish() } }
    var storeChanges: AsyncStream<StoreChanged> { AsyncStream { $0.finish() } }
    func connect() async throws {}
    func disconnect() async {}
    func deviceInfo() async throws -> DeviceInfo { DeviceInfo(name: "Trailhead", firmwareVersion: "0.4.2") }
    func readConfig() async throws -> DeviceConfig { DeviceConfig(name: "Trailhead") }
    func writeConfig(_ config: DeviceConfig) async throws {}
    func listRoutes() async throws -> [RouteCatalogEntry] { [] }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { throw DeviceError.readFailed }
    func uploadRoute(_ route: RouteBlob) -> TransferHandle { .immediatelyFinished(.failed(.notConnected)) }
    func deleteRoute(_ id: DeviceObjectID) async throws {}
    func listRides() async throws -> RideCatalog { RideCatalog(rides: []) }
    func rideDetail(_ id: RideID) async throws -> RideDetail { throw DeviceError.readFailed }
    func downloadRides(_ ids: [RideID]) -> RideDownload { .finished() }
    func readDiagnostics() async throws -> Data { Data() }

    /// A minimal valid OBCU container for the staged preview: 64-byte header + a
    /// tiny raw image, both CRCs correct.
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
