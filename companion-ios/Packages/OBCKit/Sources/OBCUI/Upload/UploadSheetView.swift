import SwiftUI
import OBCDomain
import OBCTransport

/// The upload sheet, presented over the route detail: the app never leaves the route.
/// Uploading shows the live bar, the size readout, the device-correspondence note and
/// an always-reachable Cancel; a drop swaps in the resume framing; completion holds
/// the confirm briefly, then dismisses.
///
/// Present it inside `.sheet`: the view brings its own `OBCSheetContainer` chrome and
/// detent, and drives dismissal through `model.shouldDismiss`.
public struct UploadSheetView: View {
    private let model: UploadSheetModel
    @Environment(\.dismiss) private var dismiss

    public init(model: UploadSheetModel) {
        self.model = model
    }

    public var body: some View {
        OBCSheetContainer {
            switch model.phase {
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
        // Mid-transfer the sheet owns the upload: Cancel is the escape, not an
        // accidental swipe that would silently abort or orphan the transfer.
        .interactiveDismissDisabled(model.phase == .uploading || model.phase == .interrupted)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("upload.sheet")
        .task { model.start() }
        .onChange(of: model.shouldDismiss) { _, should in
            if should { dismiss() }
        }
        .onDisappear { model.sheetDismissed() }
    }

    private var sheetHeight: CGFloat {
        switch model.phase {
        case .uploading: 280
        case .interrupted: 340
        case .done: 320
        case .failed: 310
        }
    }
    // MARK: Uploading, and its interrupted framing

    @ViewBuilder
    private func progressContent(interrupted: Bool) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .center, spacing: 13) {
                iconTile(
                    systemImage: interrupted ? "exclamationmark.triangle" : "square.and.arrow.up",
                    color: interrupted ? OBCTheme.danger : OBCTheme.secondary
                )
                VStack(alignment: .leading, spacing: 2) {
                    Text(interrupted ? "Upload interrupted" : "Uploading to \(model.deviceName)")
                        .font(.system(.callout, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                        .accessibilityIdentifier("upload.title")
                    Text(model.sizeLine)
                        .font(.system(.caption).monospacedDigit())
                        .foregroundStyle(OBCTheme.secondary)
                        .accessibilityIdentifier("upload.sizeLine")
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Text(model.percentLine)
                    .font(.system(.body, weight: .medium).monospacedDigit())
                    .foregroundStyle(interrupted ? OBCTheme.secondary : OBCTheme.ink)
                    .accessibilityIdentifier("upload.percent")
            }
            .padding(.bottom, 16)

            OBCProgressBar(value: model.fraction)

            HStack(alignment: .top, spacing: 8) {
                Text("◆")
                    .font(.system(.caption))
                    .foregroundStyle(interrupted ? OBCTheme.danger : OBCTheme.secondary)
                Text(interrupted
                    ? "The link to \(model.deviceName) dropped. What's sent is kept — resume picks up right where it left off."
                    : "Your OBC shows a matching bar. Keep it awake and nearby.")
                    .font(.system(.caption))
                    .lineSpacing(2)
                    .foregroundStyle(OBCTheme.secondary)
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

    // MARK: Done

    private var doneContent: some View {
        VStack(spacing: 0) {
            ZStack {
                Circle()
                    .fill(OBCTheme.rust)
                    .frame(width: 64, height: 64)
                    .background(Circle().fill(OBCTheme.rust.opacity(0.12)).frame(width: 80, height: 80))
                Image(systemName: "checkmark")
                    .font(.system(.title, weight: .bold))
                    .foregroundStyle(OBCTheme.onRust)
            }
            .padding(.top, 6)
            .padding(.bottom, 14)

            Text("On the device")
                .font(.system(.title3, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .accessibilityIdentifier("upload.doneTitle")
            Text("\(model.routeName) is ready to ride. It'll show under Routes on \(model.deviceName).")
                .font(.system(.footnote))
                .lineSpacing(3)
                .foregroundStyle(OBCTheme.secondary)
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

    // MARK: Failed for good, with no resume offset to continue from

    private var failedContent: some View {
        VStack(spacing: 0) {
            iconTile(
                systemImage: model.failure == .storageFull ? "externaldrive.badge.exclamationmark" : "exclamationmark.triangle",
                color: OBCTheme.danger
            )
            .padding(.top, 6)
            .padding(.bottom, 14)

            Text(model.failedTitle)
                .font(.system(.title3, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .accessibilityIdentifier("upload.failedTitle")
            Text(model.failedMessage)
                .font(.system(.footnote))
                .lineSpacing(3)
                .foregroundStyle(OBCTheme.secondary)
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

    private func iconTile(systemImage: String, color: Color) -> some View {
        Image(systemName: systemImage)
            .font(.system(.title3, weight: .medium))
            .foregroundStyle(color)
            .frame(width: 44, height: 44)
            .background(RoundedRectangle(cornerRadius: OBCTheme.radiusMedium).fill(color.opacity(0.12)))
    }
}

#if DEBUG
/// Preview-only transport whose upload pumps paced ticks, so the bar animates and
/// then holds the confirm. OBCUI cannot import OBCMock.
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
            OBCTheme.page
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

/// A real OBCR route, a short synthetic climb, so the preview's size readout shows
/// the true kB scale and not a placeholder byte count.
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
        payload: RouteObjectCodec.encode(points: points, waypoints: [waypoint], name: "Kettle Moraine Loop", bikeType: .road)
    )
}
#endif
