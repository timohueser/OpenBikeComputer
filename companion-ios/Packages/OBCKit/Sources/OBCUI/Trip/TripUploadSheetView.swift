import SwiftUI
import OBCDomain
import OBCTransport

/// The whole-trip send sheet, presented over the trip page: the route send sheet with a line
/// that names the day moving now. A drop offers Send again, which restarts that day; completion
/// shows the device with the card it puts up when a trip lands.
///
/// It sizes its detent to its content, as the route send sheet does.
public struct TripUploadSheetView: View {
    private let model: TripUploadModel
    @Environment(\.dismiss) private var dismiss
    /// The measured content height, which the detent follows.
    @State private var contentHeight: CGFloat = 260

    public init(model: TripUploadModel) {
        self.model = model
    }

    public var body: some View {
        OBCSheetContainer {
            ScrollView {
                Group {
                    switch model.phase {
                    case .uploading:
                        progressContent(interrupted: false)
                    case .interrupted:
                        progressContent(interrupted: true)
                    case .done:
                        SentToDeviceView(trip: model.card, deviceName: model.deviceName) { model.dismiss() }
                    case .failed:
                        failedContent
                    }
                }
                .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { contentHeight = $0 }
            }
            .scrollBounceBehavior(.basedOnSize)
        }
        // The container's bottom inset stands in for the home-indicator safe area, so the detent
        // is the measured content plus the grabber band and that inset.
        .ignoresSafeArea(.container, edges: .bottom)
        .presentationDetents([.height(contentHeight + 74)])
        .interactiveDismissDisabled(model.phase == .uploading || model.phase == .interrupted)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("tripUpload.sheet")
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
                    .accessibilityIdentifier("tripUpload.title")
                Text(model.percentLine)
                    .font(.obcStat(.title3))
                    .foregroundStyle(interrupted ? OBCTheme.secondary : OBCTheme.ink)
                    .accessibilityIdentifier("tripUpload.percent")
            }
            Text(model.stepProgressLabel)
                .font(.system(.subheadline).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
                .padding(.top, 2)
                .accessibilityIdentifier("tripUpload.stepLabel")
            Text(model.sizeLine)
                .font(.system(.subheadline).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
                .padding(.bottom, 14)
                .accessibilityIdentifier("tripUpload.sizeLine")

            OBCProgressBar(value: model.fraction)

            Text(interrupted
                ? "\(model.deviceName) went out of range. The days sent so far stay on it. Sending again starts this day from the beginning."
                : "The days go one at a time, then the trip. Keep \(model.deviceName) on and near your phone.")
                .font(.system(.subheadline))
                .foregroundStyle(OBCTheme.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 14)

            if interrupted {
                Button("Send again") { model.resume() }
                    .buttonStyle(.obcPrimary)
                    .accessibilityIdentifier("tripUpload.resume")
                    .padding(.top, 20)
            }
            Button("Cancel") { model.cancel() }
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("tripUpload.cancel")
                .padding(.top, interrupted ? 10 : 20)
        }
    }

    // MARK: Failed, from a precheck deficit or a device reject mid-queue

    private var failedContent: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(model.failedTitle)
                .font(.system(.title3, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .accessibilityIdentifier("tripUpload.failedTitle")
            Text(model.failedMessage)
                .font(.system(.subheadline))
                .foregroundStyle(OBCTheme.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
                .padding(.bottom, 20)
                .accessibilityIdentifier("tripUpload.failedMessage")

            Button("Close") { model.dismiss() }
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("tripUpload.close")
        }
    }
}
