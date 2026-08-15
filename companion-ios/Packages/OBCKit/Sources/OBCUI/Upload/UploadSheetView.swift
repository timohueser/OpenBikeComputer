import SwiftUI
import OBCDomain
import OBCTransport

/// The upload sheet (B5, design F/F₂) — presented over the route detail; the
/// app never leaves the route. Uploading (F) shows the live bar, the
/// plain-English size readout, the device-correspondence note, and an
/// always-reachable **Cancel upload**; a drop swaps in the resume framing;
/// completion holds the F₂ confirm briefly, then dismisses.
///
/// Present inside `.sheet` — the view brings its own `OBCSheetContainer`
/// chrome and detent, and drives dismissal through `model.shouldDismiss`.
public struct UploadSheetView: View {
    private let model: UploadSheetModel
    @Environment(\.dismiss) private var dismiss

    public init(model: UploadSheetModel) {
        self.model = model
    }

    public var body: some View {
        OBCSheetContainer {
            switch model.phase {
            case .ready:
                readyContent
            case .uploading:
                progressContent(interrupted: false)
            case .interrupted:
                progressContent(interrupted: true)
            case .done:
                doneContent
            case .failed:
                failedContent
            }
        }
        .presentationDetents([.height(sheetHeight)])
        // Mid-transfer the sheet owns the upload — Cancel is the escape, not an
        // accidental swipe that would silently abort (or orphan) the transfer.
        .interactiveDismissDisabled(model.phase == .uploading || model.phase == .interrupted)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("upload.sheet")
        .task { model.start() }
        .onChange(of: model.shouldDismiss) { _, should in
            if should { dismiss() }
        }
        .onDisappear { model.sheetDismissed() }
    }

    /// The design's sheet hugs its content — the interrupted framing carries
    /// one extra button, F₂ the taller centered confirm.
    private var sheetHeight: CGFloat {
        switch model.phase {
        case .ready: model.supportsRetention ? 320 : 260
        case .uploading: 280
        case .interrupted: 340
        case .done: 320
        case .failed: 310
        }
    }

    // MARK: Ready — the pre-transfer confirm (epic #638 S7)

    private var readyContent: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .center, spacing: 13) {
                iconTile(systemImage: "square.and.arrow.up", color: OBCTheme.forest)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Upload to \(model.deviceName)")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                        .accessibilityIdentifier("upload.readyTitle")
                    Text(model.readySizeLine)
                        .font(.obcMono(size: 12.5))
                        .foregroundStyle(OBCTheme.inkFaint)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.bottom, 16)

            // The Auto-delete row — hidden on a device without retention capability
            // (S6 flag), which also skips the `.ready` confirm entirely.
            if model.supportsRetention {
                OBCGroupedSection {
                    OBCRetentionRow(
                        selection: model.retention,
                        showsDivider: false,
                        accessibilityID: "upload.autoDelete",
                        onSelect: { model.selectRetention($0) }
                    )
                }
                .padding(.bottom, 4)
            }

            Button("Upload to \(model.deviceName)") { model.beginUpload() }
                .buttonStyle(.obcPrimary)
                .disabled(!model.canUpload)
                .accessibilityIdentifier("upload.begin")
                .padding(.top, 14)
            Button("Cancel") { model.dismiss() }
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("upload.cancel")
                .padding(.top, 10)
        }
    }

    // MARK: F — uploading (and its interrupted framing)

    @ViewBuilder
    private func progressContent(interrupted: Bool) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .center, spacing: 13) {
                iconTile(
                    systemImage: interrupted ? "exclamationmark.triangle" : "square.and.arrow.up",
                    color: interrupted ? OBCTheme.warning : OBCTheme.forest
                )
                VStack(alignment: .leading, spacing: 2) {
                    Text(interrupted ? "Upload interrupted" : "Uploading to \(model.deviceName)")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                        .accessibilityIdentifier("upload.title")
                    Text(model.sizeLine)
                        .font(.obcMono(size: 12.5))
                        .foregroundStyle(OBCTheme.inkFaint)
                        .accessibilityIdentifier("upload.sizeLine")
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Text(model.percentLine)
                    .font(.obcMono(size: 18, weight: .medium))
                    .foregroundStyle(interrupted ? OBCTheme.inkFaint : OBCTheme.forest)
                    .accessibilityIdentifier("upload.percent")
            }
            .padding(.bottom, 16)

            OBCProgressBar(value: model.fraction)

            HStack(alignment: .top, spacing: 8) {
                Text("◆")
                    .font(.system(size: 12.5))
                    .foregroundStyle(interrupted ? OBCTheme.warning : OBCTheme.amber)
                Text(interrupted
                    ? "The link to \(model.deviceName) dropped. What's sent is kept — resume picks up right where it left off."
                    : "Your OBC shows a matching bar. Keep it awake and nearby.")
                    .font(.system(size: 12.5))
                    .lineSpacing(2)
                    .foregroundStyle(OBCTheme.inkFaint)
            }
            .padding(.top, 13)

            if interrupted {
                Button("Resume upload") { model.resume() }
                    .buttonStyle(.obcPrimary)
                    .accessibilityIdentifier("upload.resume")
                    .padding(.top, 18)
                Button("Cancel upload") { model.cancel() }
                    .buttonStyle(.obcGhost)
                    .accessibilityIdentifier("upload.cancel")
                    .padding(.top, 10)
            } else {
                Button("Cancel upload") { model.cancel() }
                    .buttonStyle(.obcGhost)
                    .accessibilityIdentifier("upload.cancel")
                    .padding(.top, 18)
            }
        }
    }

    // MARK: F₂ — done

    private var doneContent: some View {
        VStack(spacing: 0) {
            ZStack {
                Circle()
                    .fill(OBCTheme.forest)
                    .frame(width: 64, height: 64)
                    .background(Circle().fill(OBCTheme.forest.opacity(0.12)).frame(width: 80, height: 80))
                Image(systemName: "checkmark")
                    .font(.system(size: 28, weight: .bold))
                    .foregroundStyle(.white)
            }
            .padding(.top, 6)
            .padding(.bottom, 14)

            Text("On the device")
                .font(.obcSerif(size: 20))
                .foregroundStyle(OBCTheme.ink)
                .accessibilityIdentifier("upload.doneTitle")
            Text("\(model.routeName) is ready to ride. It'll show under Routes on \(model.deviceName).")
                .font(.system(size: 13.5))
                .lineSpacing(3)
                .foregroundStyle(OBCTheme.inkSoft)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 260)
                .padding(.top, 6)
                .padding(.bottom, 18)

            Button("Done") { model.dismiss() }
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("upload.done")
        }
        .frame(maxWidth: .infinity)
    }

    // MARK: Failed for good (no resume offset to continue from)

    private var failedContent: some View {
        VStack(spacing: 0) {
            iconTile(
                systemImage: model.failure == .storageFull ? "externaldrive.badge.exclamationmark" : "exclamationmark.triangle",
                color: OBCTheme.warning
            )
            .padding(.top, 6)
            .padding(.bottom, 14)

            Text(model.failedTitle)
                .font(.obcSerif(size: 20))
                .foregroundStyle(OBCTheme.ink)
                .accessibilityIdentifier("upload.failedTitle")
            Text(model.failedMessage)
                .font(.system(size: 13.5))
                .lineSpacing(3)
                .foregroundStyle(OBCTheme.inkSoft)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 260)
                .padding(.top, 6)
                .padding(.bottom, 18)
                .accessibilityIdentifier("upload.failedMessage")

            Button("Close") { model.dismiss() }
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("upload.close")
        }
        .frame(maxWidth: .infinity)
    }

    // MARK: Pieces

    private func iconTile(systemImage: String, color: Color) -> some View {
        Image(systemName: systemImage)
            .font(.system(size: 20, weight: .medium))
            .foregroundStyle(color)
            .frame(width: 44, height: 44)
            .background(RoundedRectangle(cornerRadius: OBCTheme.radiusMedium).fill(color.opacity(0.12)))
    }
}

#if DEBUG
/// Preview-only transport whose upload pumps paced ticks (OBCUI can't import
/// OBCMock) — F animates, then holds F₂.
private struct PreviewUploadTransport: DeviceLink, DeviceObjects {
    var dropAt: Double?

    var state: AsyncStream<ConnectionState> { AsyncStream { _ in } }
    func connect() async throws {}
    func disconnect() async {}
    func deviceInfo() async throws -> DeviceInfo { DeviceInfo(name: "Trailhead", firmwareVersion: "0") }
    func listRoutes() async throws -> [RouteCatalogEntry] { [] }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { throw DeviceError.readFailed }
    func deleteRoute(_ id: DeviceObjectID) async throws {}
    func listRides() async throws -> RideCatalog { RideCatalog(rides: []) }
    func rideDetail(_ id: RideID) async throws -> RideDetail { throw DeviceError.readFailed }
    func downloadRides(_ ids: [RideID]) -> RideDownload { .finished() }

    func uploadRoute(_ route: RouteBlob) -> TransferHandle {
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        let outcome = AsyncPromise<TransferOutcome>()
        let total = route.payload.count
        Task {
            for step in 1...100 {
                try? await Task.sleep(for: .milliseconds(40))
                let done = total * step / 100
                continuation.yield(TransferProgress(bytesDone: done, total: total))
            }
            continuation.finish()
            outcome.fulfill(.completed)
        }
        return TransferHandle(progress: stream, outcome: outcome, onCancel: {}, onResume: {})
    }
}

#Preview("F → F₂ · live") {
    struct Demo: View {
        @State private var shown = true
        var body: some View {
            OBCTheme.parchment
                .ignoresSafeArea()
                .sheet(isPresented: $shown) {
                    UploadSheetView(model: UploadSheetModel(
                        transport: PreviewUploadTransport(),
                        blob: previewBlob,
                        deviceName: "Trailhead"
                    ))
                }
        }
    }
    return Demo()
}

/// A real OBCR route (a short synthetic climb) so the preview's size readout shows
/// the true kB scale, not a placeholder byte count.
private var previewBlob: RouteBlob {
    let waypoint = Waypoint(
        index: 0, name: "Ottawa Lake trailhead",
        distanceAlongMeters: 0, coordinate: Coordinate(latitude: 43.02, longitude: -88.55)
    )
    let points = (0..<400).map { i in
        RoutePoint(
            coordinate: Coordinate(latitude: 43.02 + 0.0006 * Double(i), longitude: -88.55 + 0.0004 * Double(i % 2)),
            elevationMeters: 280 + Double(i % 40)
        )
    }
    return RouteBlob(
        summary: RouteSummary(
            id: RouteID("preview"), name: "Kettle Moraine Loop",
            distanceMeters: 62_400, elevationGainMeters: 840
        ),
        waypoints: [waypoint],
        payload: RouteObjectCodec.encode(points: points, waypoints: [waypoint], name: "Kettle Moraine Loop")
    )
}
#endif
