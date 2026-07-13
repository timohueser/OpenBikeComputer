import SwiftUI
import UniformTypeIdentifiers

/// **Import Button & Files Picker** (§9) — the large-title `+` button (I1):
/// opens the system document picker directly, filtered to the supported route
/// extensions (pass `RouteImporter.supportedFileExtensions` from the
/// composition root so the filter always matches the registered decoders).
///
/// Deliberately not a menu: with one in-app action, an intermediate popover is
/// a dead click (share-from-another-app arrives via the registered document
/// types → `onOpenURL`, not from here).
public struct OBCImportButton: View {
    let fileExtensions: Set<String>
    let onPick: (URL) -> Void
    @State private var pickerShown = false

    public init(fileExtensions: Set<String>, onPick: @escaping (URL) -> Void) {
        self.fileExtensions = fileExtensions
        self.onPick = onPick
    }

    private var contentTypes: [UTType] {
        // Ad-hoc file types: UTType(filenameExtension:) covers gpx/tcx without
        // the app having to declare imported type identifiers.
        fileExtensions.sorted().compactMap { UTType(filenameExtension: $0) }
    }

    public var body: some View {
        Button {
            pickerShown = true
        } label: {
            Image(systemName: "plus")
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(OBCTheme.tint)
                .frame(width: 34, height: 34)
                .background(OBCTheme.panel)
                .clipShape(Circle())
                .overlay(Circle().strokeBorder(OBCTheme.line))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Import a route")
        .fileImporter(
            isPresented: $pickerShown,
            allowedContentTypes: contentTypes
        ) { result in
            if case .success(let url) = result { onPick(url) }
        }
    }
}

#Preview("Import button") {
    HStack {
        Text("Routes").font(.obcSerif(size: 32)).foregroundStyle(OBCTheme.ink)
        Spacer()
        OBCImportButton(fileExtensions: ["gpx", "tcx"]) { _ in }
    }
    .padding(20)
    .background(OBCTheme.parchment)
}
