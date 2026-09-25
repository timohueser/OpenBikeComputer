import SwiftUI
import OBCDomain
import OBCTransport

/// The send sheet, presented over the route page: the app never leaves the route. Sending shows
/// the live bar and an always-reachable Cancel; a drop offers Send again; completion shows the
/// device with the route on its screen.
///
/// Present it inside `.sheet`: the view brings its own `OBCSheetContainer` chrome, sizes its detent
/// to its content, and drives dismissal through `model.shouldDismiss`.
public struct UploadSheetView: View {
    private let model: UploadSheetModel
    @Environment(\.dismiss) private var dismiss

    public init(model: UploadSheetModel) {
        self.model = model
    }

    public var body: some View {
        OBCSheetContainer {
            Group {
                switch model.phase {
                case .uploading:
                    progressContent(interrupted: false)
                case .interrupted:
                    progressContent(interrupted: true)
                case .done:
                    SentToDeviceView(overview: model.overview, deviceName: model.deviceName) {
                        model.dismiss()
                    }
                case .failed:
                    failedContent
                }
            }
        }
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

    // MARK: Sending, and its interrupted framing

    @ViewBuilder
    private func progressContent(interrupted: Bool) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text(interrupted ? "Connection lost" : "Sending to \(model.deviceName)")
                    .font(.system(.title3, weight: .bold))
                    .foregroundStyle(OBCTheme.ink)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .accessibilityIdentifier("upload.title")
                Text(model.percentLine)
                    .font(.obcStat(.title3))
                    .foregroundStyle(interrupted ? OBCTheme.secondary : OBCTheme.ink)
                    .accessibilityIdentifier("upload.percent")
            }
            Text(model.sizeLine)
                .font(.system(.subheadline).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
                .padding(.top, 2)
                .padding(.bottom, 14)
                .accessibilityIdentifier("upload.sizeLine")

            OBCProgressBar(value: model.fraction)

            Text(interrupted
                ? "\(model.deviceName) went out of range. Sending again starts from the beginning."
                : "\(model.deviceName) shows the same progress. Keep it on and near your phone.")
                .font(.system(.subheadline))
                .foregroundStyle(OBCTheme.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 14)

            if interrupted {
                Button("Send again") { model.resume() }
                    .buttonStyle(.obcPrimary)
                    .accessibilityIdentifier("upload.resume")
                    .padding(.top, 20)
            }
            Button("Cancel") { model.cancel() }
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("upload.cancel")
                .padding(.top, interrupted ? 10 : 20)
        }
    }

    // MARK: Failed for good, with no transfer to continue

    private var failedContent: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(model.failedTitle)
                .font(.system(.title3, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .accessibilityIdentifier("upload.failedTitle")
            Text(model.failedMessage)
                .font(.system(.subheadline))
                .foregroundStyle(OBCTheme.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
                .padding(.bottom, 20)
                .accessibilityIdentifier("upload.failedMessage")

            Button("Send again") { model.retry() }
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("upload.retry")
                .padding(.bottom, 10)
            Button("Close") { model.dismiss() }
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("upload.close")
        }
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
