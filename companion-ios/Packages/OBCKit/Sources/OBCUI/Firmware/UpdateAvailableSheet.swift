import SwiftUI
import OBCTransport

/// The launch sheet: what a rider sees when a published firmware update they have not
/// been offered is waiting and they open the app.
///
/// It says which version, how big and what changed, and offers two answers. View
/// pushes the firmware-update screen, where the download, verify and send already
/// live; nothing is downloaded from here. Not now is a real answer, not a snooze:
/// this version is not raised again, and a newer one is.
public struct UpdateAvailableSheet: View {
    private let update: UpdateSurfaceModel.PendingUpdate
    private let onView: () -> Void
    private let onNotNow: () -> Void

    @Environment(\.openURL) private var openURL

    public init(
        update: UpdateSurfaceModel.PendingUpdate,
        onView: @escaping () -> Void,
        onNotNow: @escaping () -> Void
    ) {
        self.update = update
        self.onView = onView
        self.onNotNow = onNotNow
    }

    public var body: some View {
        OBCSheetContainer {
            VStack(alignment: .leading, spacing: 16) {
                Text("Firmware update available")
                    .font(.system(.title2, weight: .bold))
                    .foregroundStyle(OBCTheme.ink)

                HStack(spacing: 12) {
                    OBCIconTile(systemImage: "arrow.down.to.line", color: OBCTheme.tint)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(versionLine)
                            .font(.system(.callout, weight: .semibold))
                            .foregroundStyle(OBCTheme.ink)
                        Text(sizeLine)
                            .font(.system(.caption).monospacedDigit())
                            .foregroundStyle(OBCTheme.secondary)
                    }
                }
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("firmware.updateSheet.release")

                Text("It installs only after you confirm on \(update.deviceName).")
                    .font(.system(.subheadline))
                    .foregroundStyle(OBCTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                if let notes = update.release.notesURL {
                    Button("Release notes") { openURL(notes) }
                        .font(.system(.subheadline, weight: .semibold))
                        .foregroundStyle(OBCTheme.tint)
                        .frame(minHeight: 44)
                        .accessibilityIdentifier("firmware.updateSheet.releaseNotes")
                }

                VStack(spacing: 10) {
                    Button("View") { onView() }
                        .buttonStyle(.obcPrimary)
                        .accessibilityIdentifier("firmware.updateSheet.view")
                    Button("Not now") { onNotNow() }
                        .buttonStyle(.obcGhost)
                        .accessibilityIdentifier("firmware.updateSheet.notNow")
                }
                .padding(.top, 2)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .accessibilityIdentifier("firmware.updateSheet")
    }

    /// The same versioned readout the firmware-update screen uses.
    private var versionLine: String { UpdateNoticeCopy.versioned(update.release.version) }

    /// The container's size, so the rider knows what the download costs before tapping
    /// into a screen that offers to make it.
    private var sizeLine: String {
        ByteCountFormatter.string(fromByteCount: Int64(update.release.bytes), countStyle: .file)
    }
}

#if DEBUG
#Preview("Update available") {
    struct Demo: View {
        @State private var shown = true
        var body: some View {
            OBCTheme.page
                .ignoresSafeArea()
                .sheet(isPresented: $shown) {
                    UpdateAvailableSheet(
                        update: UpdateSurfaceModel.PendingUpdate(
                            release: FirmwareRelease(
                                version: "1.4.0",
                                bytes: 874_496,
                                sha256: String(repeating: "a", count: 64),
                                url: URL(string: "https://updates.openbikecomputer.com/fw/UPDATE.BIN")!,
                                notes: "https://github.com/timohueser/OpenBikeComputer/releases"
                            ),
                            deviceName: "Trailhead"
                        ),
                        onView: {},
                        onNotNow: { shown = false }
                    )
                }
        }
    }
    return Demo()
}
#endif
