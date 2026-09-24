import SwiftUI
import UniformTypeIdentifiers

/// The large-title `+` button, the screen's one amber action: it opens the system document picker directly, filtered
/// to the supported route extensions. Pass `RouteImporter.supportedFileExtensions`
/// from the composition root so the filter always matches the registered decoders.
///
/// It is deliberately not a menu: with one in-app action an intermediate popover is a
/// dead click, and a share from another app arrives through `onOpenURL`.
public struct OBCImportButton: View {
    let fileExtensions: Set<String>
    let onPick: ([URL]) -> Void
    @State private var pickerShown = false

    public init(fileExtensions: Set<String>, onPick: @escaping ([URL]) -> Void) {
        self.fileExtensions = fileExtensions
        self.onPick = onPick
    }

    private var contentTypes: [UTType] {
        // Ad-hoc file types: UTType(filenameExtension:) covers gpx and tcx without the
        // app having to declare imported type identifiers.
        fileExtensions.sorted().compactMap { UTType(filenameExtension: $0) }
    }

    public var body: some View {
        Button {
            pickerShown = true
        } label: {
            Image(systemName: "plus")
                .font(.system(.title3, weight: .semibold))
                .foregroundStyle(OBCTheme.onAmber)
                .frame(width: 44, height: 44)
                .background(OBCTheme.amber, in: Circle())
                .obcFixedGeometryType()
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Import a route")
        .fileImporter(
            isPresented: $pickerShown,
            allowedContentTypes: contentTypes,
            allowsMultipleSelection: true
        ) { result in
            if case .success(let urls) = result, !urls.isEmpty { onPick(urls) }
        }
    }
}

#Preview("Import button") {
    HStack {
        Text("Routes").font(.system(.largeTitle, weight: .bold)).foregroundStyle(OBCTheme.ink)
        Spacer()
        OBCImportButton(fileExtensions: ["gpx", "tcx"]) { _ in }
    }
    .padding(20)
    .background(OBCTheme.page)
}
