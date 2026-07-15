import SwiftUI
import OBCDomain
import OBCTransport

/// The **whole-trip upload sheet** (TR8) — the queued mode of the upload sheet
/// (design F/F₂), presented over the trip page. It's the single-route sheet with
/// a "Stage X of Y — <name>" header over the per-transfer bar, and a
/// skipped/committed tally in the done state. Interruption and cancel read the
/// same as a single upload (uploads restart, not resume).
public struct TripUploadSheetView: View {
    private let model: TripUploadModel
    @Environment(\.dismiss) private var dismiss

    public init(model: TripUploadModel) {
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
        case .ready: 330
        case .uploading: 300
        case .interrupted: 360
        case .done: 330
        case .failed: 320
        }
    }

    // MARK: Ready — the pre-transfer confirm (epic #638)

    /// The Auto-delete confirm for the whole trip: the trip + the retention row
    /// (the trip's level applies to every member route), then **Upload trip**. The
    /// queue starts on that tap. Only reached on a retention-capable device — an
    /// incapable one skips straight to `.uploading` (the row hidden).
    private var readyContent: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .center, spacing: 13) {
                iconTile(systemImage: "square.and.arrow.up", color: OBCTheme.forest)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Upload to \(model.deviceName)")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                        .accessibilityIdentifier("tripUpload.readyTitle")
                    Text(model.tripName)
                        .font(.obcMono(size: 12.5))
                        .foregroundStyle(OBCTheme.inkFaint)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.bottom, 16)

            // The Auto-delete row — the whole trip's level, applied to every member
            // route. Hidden on a device without retention capability (which also
            // skips this confirm entirely).
            if model.supportsRetention {
                OBCGroupedSection {
                    OBCRetentionRow(
                        selection: model.retention,
                        showsDivider: false,
                        accessibilityID: "tripUpload.autoDelete",
                        onSelect: { model.selectRetention($0) }
                    )
                }
                .padding(.bottom, 4)
            }

            Button("Upload trip") { model.beginUpload() }
                .buttonStyle(.obcPrimary)
                .disabled(!model.canUpload)
                .accessibilityIdentifier("tripUpload.begin")
                .padding(.top, 14)
            Button("Cancel") { model.dismiss() }
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("tripUpload.cancelReady")
                .padding(.top, 10)
        }
    }

    // MARK: F — uploading (queued)

    @ViewBuilder
    private func progressContent(interrupted: Bool) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .center, spacing: 13) {
                iconTile(
                    systemImage: interrupted ? "exclamationmark.triangle" : "square.and.arrow.up",
                    color: interrupted ? OBCTheme.warning : OBCTheme.forest
                )
                VStack(alignment: .leading, spacing: 2) {
                    Text(interrupted ? "Upload interrupted" : "Uploading \(model.tripName)")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                        .accessibilityIdentifier("tripUpload.title")
                    Text(model.sizeLine)
                        .font(.obcMono(size: 12.5))
                        .foregroundStyle(OBCTheme.inkFaint)
                        .accessibilityIdentifier("tripUpload.sizeLine")
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Text(model.percentLine)
                    .font(.obcMono(size: 18, weight: .medium))
                    .foregroundStyle(interrupted ? OBCTheme.inkFaint : OBCTheme.forest)
                    .accessibilityIdentifier("tripUpload.percent")
            }
            .padding(.bottom, 10)

            // The queued-mode header: which stage of how many is moving now.
            Text(model.stageProgressLabel)
                .font(.obcMono(size: 12))
                .foregroundStyle(OBCTheme.forest)
                .accessibilityIdentifier("tripUpload.stageLabel")
                .padding(.bottom, 12)

            OBCProgressBar(value: model.fraction)

            HStack(alignment: .top, spacing: 8) {
                Text("◆")
                    .font(.system(size: 12.5))
                    .foregroundStyle(interrupted ? OBCTheme.warning : OBCTheme.amber)
                Text(interrupted
                    ? "The link to \(model.deviceName) dropped. Finished stages are kept — resume restarts this one."
                    : "Sending each stage in order, then the trip. Keep \(model.deviceName) awake and nearby.")
                    .font(.system(size: 12.5))
                    .lineSpacing(2)
                    .foregroundStyle(OBCTheme.inkFaint)
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

    // MARK: F₂ — done (with the tally)

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

            Text("Trip on the device")
                .font(.obcSerif(size: 20))
                .foregroundStyle(OBCTheme.ink)
                .accessibilityIdentifier("tripUpload.doneTitle")
            Text("\(model.tripName) is ready to ride. It'll show as a folder under Routes on \(model.deviceName).")
                .font(.system(size: 13.5))
                .lineSpacing(3)
                .foregroundStyle(OBCTheme.inkSoft)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 260)
                .padding(.top, 6)
                .padding(.bottom, 4)
            Text(model.doneTally)
                .font(.obcMono(size: 12))
                .foregroundStyle(OBCTheme.inkFaint)
                .accessibilityIdentifier("tripUpload.doneTally")
                .padding(.bottom, 18)

            Button("Done") { model.dismiss() }
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("tripUpload.done")
        }
        .frame(maxWidth: .infinity)
    }

    // MARK: Failed (precheck deficit, or a device reject mid-queue)

    private var failedContent: some View {
        VStack(spacing: 0) {
            iconTile(
                systemImage: "externaldrive.badge.exclamationmark",
                color: OBCTheme.warning
            )
            .padding(.top, 6)
            .padding(.bottom, 14)

            Text(model.failedTitle)
                .font(.obcSerif(size: 20))
                .foregroundStyle(OBCTheme.ink)
                .accessibilityIdentifier("tripUpload.failedTitle")
            Text(model.failedMessage)
                .font(.system(size: 13.5))
                .lineSpacing(3)
                .foregroundStyle(OBCTheme.inkSoft)
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
            .font(.system(size: 20, weight: .medium))
            .foregroundStyle(color)
            .frame(width: 44, height: 44)
            .background(RoundedRectangle(cornerRadius: OBCTheme.radiusMedium).fill(color.opacity(0.12)))
    }
}
