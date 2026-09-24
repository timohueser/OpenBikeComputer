import SwiftUI
import OBCDomain
import OBCTransport

/// The whole-trip upload sheet: the queued mode of the upload sheet, presented over
/// the trip page. It is the single-route sheet with a "Step X of Y" header over the
/// per-transfer bar, and a skipped and committed tally in the done state. Interruption
/// and cancel read the same as a single upload: uploads restart, they do not resume.
public struct TripUploadSheetView: View {
    private let model: TripUploadModel
    @Environment(\.dismiss) private var dismiss

    public init(model: TripUploadModel) {
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
        .interactiveDismissDisabled(model.phase == .uploading || model.phase == .interrupted)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("tripUpload.sheet")
        .task { model.start() }
        .onChange(of: model.shouldDismiss) { _, should in
            if should { dismiss() }
        }
        .onDisappear { model.sheetDismissed() }
    }

    private var sheetHeight: CGFloat {
        switch model.phase {
        case .uploading: 300
        case .interrupted: 360
        case .done: 330
        case .failed: 320
        }
    }
    // MARK: Uploading the queue

    @ViewBuilder
    private func progressContent(interrupted: Bool) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .center, spacing: 13) {
                iconTile(
                    systemImage: interrupted ? "exclamationmark.triangle" : "square.and.arrow.up",
                    color: interrupted ? OBCTheme.danger : OBCTheme.secondary
                )
                VStack(alignment: .leading, spacing: 2) {
                    Text(interrupted ? "Upload interrupted" : "Uploading \(model.tripName)")
                        .font(.system(.callout, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                        .accessibilityIdentifier("tripUpload.title")
                    Text(model.sizeLine)
                        .font(.system(.caption).monospacedDigit())
                        .foregroundStyle(OBCTheme.secondary)
                        .accessibilityIdentifier("tripUpload.sizeLine")
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Text(model.percentLine)
                    .font(.system(.body, weight: .medium).monospacedDigit())
                    .foregroundStyle(interrupted ? OBCTheme.secondary : OBCTheme.ink)
                    .accessibilityIdentifier("tripUpload.percent")
            }
            .padding(.bottom, 10)

            // The queued-mode header: which step of how many is moving now.
            Text(model.stepProgressLabel)
                .font(.system(.caption).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
                .accessibilityIdentifier("tripUpload.stepLabel")
                .padding(.bottom, 12)

            OBCProgressBar(value: model.fraction)

            HStack(alignment: .top, spacing: 8) {
                Text("◆")
                    .font(.system(.caption))
                    .foregroundStyle(interrupted ? OBCTheme.danger : OBCTheme.secondary)
                Text(interrupted
                    ? "The link to \(model.deviceName) dropped. Finished days are kept — resume restarts this one."
                    : "Sending each day in order, then the trip. Keep \(model.deviceName) awake and nearby.")
                    .font(.system(.caption))
                    .lineSpacing(2)
                    .foregroundStyle(OBCTheme.secondary)
            }
            .padding(.top, 13)

            if interrupted {
                Button("Resume upload") { model.resume() }
                    .buttonStyle(.obcPrimary)
                    .accessibilityIdentifier("tripUpload.resume")
                    .padding(.top, 18)
                Button("Cancel upload") { model.cancel() }
                    .buttonStyle(.obcGhost)
                    .accessibilityIdentifier("tripUpload.cancel")
                    .padding(.top, 10)
            } else {
                Button("Cancel upload") { model.cancel() }
                    .buttonStyle(.obcGhost)
                    .accessibilityIdentifier("tripUpload.cancel")
                    .padding(.top, 18)
            }
        }
    }

    // MARK: Done, with the tally

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

            Text("Trip on the device")
                .font(.system(.title3, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .accessibilityIdentifier("tripUpload.doneTitle")
            Text("\(model.tripName) is ready to ride. It'll show as a folder under Routes on \(model.deviceName).")
                .font(.system(.footnote))
                .lineSpacing(3)
                .foregroundStyle(OBCTheme.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 260)
                .padding(.top, 6)
                .padding(.bottom, 4)
            Text(model.doneTally)
                .font(.system(.caption).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
                .accessibilityIdentifier("tripUpload.doneTally")
                .padding(.bottom, 18)

            Button("Done") { model.dismiss() }
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("tripUpload.done")
        }
        .frame(maxWidth: .infinity)
    }

    // MARK: Failed, from a precheck deficit or a device reject mid-queue

    private var failedContent: some View {
        VStack(spacing: 0) {
            iconTile(
                systemImage: "externaldrive.badge.exclamationmark",
                color: OBCTheme.danger
            )
            .padding(.top, 6)
            .padding(.bottom, 14)

            Text(model.failedTitle)
                .font(.system(.title3, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .accessibilityIdentifier("tripUpload.failedTitle")
            Text(model.failedMessage)
                .font(.system(.footnote))
                .lineSpacing(3)
                .foregroundStyle(OBCTheme.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 270)
                .padding(.top, 6)
                .padding(.bottom, 18)
                .accessibilityIdentifier("tripUpload.failedMessage")

            Button("Close") { model.dismiss() }
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("tripUpload.close")
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
