import SwiftUI
import UniformTypeIdentifiers

/// **Import Menu & Files Picker** (§9, NEW) — the large-title `+` button
/// (I1): a popover menu whose *Import from Files* opens the system document
/// picker filtered to the supported route extensions (`.gpx` / `.tcx` — pass
/// `RouteImporter.supportedFileExtensions` from the composition root so the
/// filter always matches the registered decoders). In-app import; no share
/// sheet required.
public struct OBCImportMenuButton: View {
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
        Menu {
            Button {
                pickerShown = true
            } label: {
                Label("Import from Files", systemImage: "folder")
            }
            Text("…or share a route to the app from Komoot, Strava, or any app.")
        } label: {
            Image(systemName: "plus")
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(OBCTheme.tint)
                .frame(width: 34, height: 34)
                .background(OBCTheme.panel)
                .clipShape(Circle())
                .overlay(Circle().strokeBorder(OBCTheme.line))
        }
        .accessibilityLabel("Import a route")
        .fileImporter(
            isPresented: $pickerShown,
            allowedContentTypes: contentTypes
        ) { result in
            if case .success(let url) = result { onPick(url) }
        }
    }
}

#Preview("Import menu") {
    HStack {
        Text("Routes").font(.obcSerif(size: 32)).foregroundStyle(OBCTheme.ink)
        Spacer()
        OBCImportMenuButton(fileExtensions: ["gpx", "tcx"]) { _ in }
    }
    .padding(20)
    .background(OBCTheme.parchment)
}
