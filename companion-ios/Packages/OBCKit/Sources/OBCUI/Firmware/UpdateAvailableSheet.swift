import SwiftUI
import OBCTransport

/// The launch sheet (#773 U5): what a rider sees when a firmware update they haven't been offered
/// is published and they open the app.
///
/// It says the three things worth knowing — which version, how big, what changed — and offers two
/// answers. **View** pushes the S7 screen, which is where the actual work (download, verify, send,
/// confirm on the glass) already lives; nothing is downloaded from here. **Not now** is a real
/// answer, not a snooze: this version won't be raised again, and a newer one will.
///
/// Deliberately spare. It interrupts an app launch, so it earns its place by being short.
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
                    .font(.obcSerif(size: 22))
                    .foregroundStyle(OBCTheme.ink)

                HStack(spacing: 12) {
                    OBCIconTile(systemImage: "sparkles", color: OBCTheme.amber)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(versionLine)
                            .font(.system(size: 16, weight: .semibold))
                            .foregroundStyle(OBCTheme.ink)
                        Text(sizeLine)
                            .font(.obcMono(size: 12))
                            .foregroundStyle(OBCTheme.inkFaint)
                    }
                }
                .accessibilityIdentifier("firmware.updateSheet.release")

                Text("Published for \(update.deviceName). You send it from the firmware screen, "
                    + "and it installs only after you confirm it on the device.")
                    .font(.system(size: 14))
                    .foregroundStyle(OBCTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)

                if let notes = update.release.notesURL {
                    Button("Release notes") { openURL(notes) }
                        .font(.system(size: 14, weight: .medium))
                        .foregroundStyle(OBCTheme.water)
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

    /// "v1.4.0" — the same versioned readout the S7 screen uses.
    private var versionLine: String { UpdateNoticeCopy.versioned(update.release.version) }

    /// "854 KB" — the container's size, so the rider knows what the download costs before tapping
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
            OBCTheme.parchment
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
                    .presentationDetents([.height(400)])
                }
        }
    }
    return Demo()
}
#endif
