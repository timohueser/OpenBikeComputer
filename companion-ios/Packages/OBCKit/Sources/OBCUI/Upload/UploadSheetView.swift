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
