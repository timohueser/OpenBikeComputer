import SwiftUI
import UniformTypeIdentifiers
import OBCDomain
import OBCTransport

/// The firmware-update screen (S7) — pushed from Settings' Firmware group. Shows
/// the running version, the staged file (version + size), and a single action:
/// "Send to bike computer". After sending, the rider confirms on the device and
/// it restarts; the screen tracks that through to the reconnect.
///
/// Import is Files-only (`UPDATE.BIN` / `.bin`), validated in the picker — a
/// corrupt file is refused here, never streamed to the device. No release feed,
/// no auto-update: the trust model is "the user chose this file".
public struct FirmwareUpdateView: View {
    @Bindable private var model: FirmwareUpdateModel
    @State private var pickerShown = false

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
                stagedGroup
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
    }

    // MARK: Running version

    private var runningGroup: some View {
        OBCGroupedSection("On the device") {
            OBCListRow(
                icon: "cpu",
                iconColor: OBCTheme.forest,
                label: "Firmware version",
                value: model.connection == .connected ? model.runningVersionLine : "—",
                showsDivider: false
            )
        }
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
    func listRides() async throws -> [RideSummary] { [] }
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
