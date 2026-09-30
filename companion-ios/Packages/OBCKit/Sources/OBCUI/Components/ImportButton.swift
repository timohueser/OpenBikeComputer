import SwiftUI
import UniformTypeIdentifiers

/// The large-title `+` button, the screen's one amber action. Without a planner it opens the
/// system document picker directly, filtered to the supported route extensions; with one it is
/// a menu of the two ways to add a route. Pass `RouteImporter.supportedFileExtensions` from the
/// composition root so the filter always matches the registered decoders. A share from another
/// app arrives through `onOpenURL`.
public struct OBCImportButton: View {
    let fileExtensions: Set<String>
    let onPick: ([URL]) -> Void
    let onNewRoute: (() -> Void)?
    @State private var pickerShown = false

    public init(fileExtensions: Set<String>, onPick: @escaping ([URL]) -> Void, onNewRoute: (() -> Void)? = nil) {
        self.fileExtensions = fileExtensions
        self.onPick = onPick
        self.onNewRoute = onNewRoute
    }

    private var contentTypes: [UTType] {
        // Ad-hoc file types: UTType(filenameExtension:) covers gpx and tcx without the
        // app having to declare imported type identifiers.
        fileExtensions.sorted().compactMap { UTType(filenameExtension: $0) }
    }

    public var body: some View {
        Group {
            if let onNewRoute {
                Menu {
                    Button("New route", systemImage: "point.topleft.down.to.point.bottomright.curvepath", action: onNewRoute)
                        .accessibilityIdentifier("main.newRoute")
                    Button("Import a file", systemImage: "doc") { pickerShown = true }
                        .accessibilityIdentifier("main.importFile")
                } label: { plus }
                .accessibilityLabel("Add a route")
            } else {
                Button { pickerShown = true } label: { plus }
                    .accessibilityLabel("Import a route")
            }
        }
        .buttonStyle(.plain)
        .fileImporter(
            isPresented: $pickerShown,
            allowedContentTypes: contentTypes,
            allowsMultipleSelection: true
        ) { result in
            if case .success(let urls) = result, !urls.isEmpty { onPick(urls) }
        }
    }

    private var plus: some View {
        Image(systemName: "plus")
            .font(.system(.title3, weight: .semibold))
            .foregroundStyle(OBCTheme.onAmber)
            .frame(width: 44, height: 44)
            .background(OBCTheme.amber, in: Circle())
            .obcFixedGeometryType()
    }
}

#Preview("Import button") {
    HStack {
        Text("Library").font(.system(.largeTitle, weight: .bold)).foregroundStyle(OBCTheme.ink)
        Spacer()
        OBCImportButton(fileExtensions: ["gpx", "tcx"]) { _ in }
    }
    .padding(20)
    .background(OBCTheme.page)
}
