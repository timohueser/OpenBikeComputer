import SwiftUI
import OBCDomain
import OBCTransport
import OBCFormats
import OBCUI

/// The app's root: the B2 launch gate (bond check → quiet reconnect, or the
/// D1–D5 pairing flow) in front of the main screen (B3), which pushes the B4
/// detail screens. Holds only the seams the composition root chose —
/// `any DeviceTransport` + `any BondStore` — plus the file-format edge
/// (`RouteImporter`) that turns a picked file into the E1 import landing.
struct RootView: View {
    @State private var launchModel: LaunchFlowModel
    @State private var mainModel: MainScreenModel
    @State private var path: [MainDestination] = []
    @State private var pendingImport: PendingImport?
    @State private var importFailedToast = false

    private let transport: any DeviceTransport
    /// The registered import formats. GPX landed with B4; TCX + the share-sheet
    /// entry are B6's.
    private let importer = RouteImporter(decoders: [GPXRouteDecoder()])
    /// A route file handed in at launch (`-OBCImportSample`) — opens E1 as soon
    /// as the main screen is up.
    private let importAtLaunch: (data: Data, fileName: String)?

    init(
        transport: any DeviceTransport,
        bondStore: any BondStore,
        importAtLaunch: (data: Data, fileName: String)? = nil
    ) {
        self.transport = transport
        self.importAtLaunch = importAtLaunch
        _launchModel = State(initialValue: LaunchFlowModel(transport: transport, bondStore: bondStore))
        _mainModel = State(initialValue: MainScreenModel(transport: transport))
    }

    var body: some View {
        LaunchFlowView(model: launchModel) {
            NavigationStack(path: $path) {
                MainScreenView(
                    model: mainModel,
                    importFileExtensions: importer.supportedFileExtensions,
                    onImportFile: { url in importFile(at: url) },
                    onSelectRoute: { route in
                        path.append(.route(id: route.id))
                    },
                    onSelectRide: { ride in
                        path.append(.ride(id: ride.id))
                    },
                    onSettings: {
                        // TODO(B8): the settings screen (G).
                    }
                )
                // Back label for the pushed details — the main screen draws its
                // own chrome, but the title still names the pop target ("‹ Routes").
                .navigationTitle("Routes")
                .navigationDestination(for: MainDestination.self) { destination in
                    detailScreen(for: destination)
                }
            }
            .obcToast(
                isPresented: $importFailedToast,
                message: "Couldn't read that file. OBC imports GPX route files."
            )
            .fullScreenCover(item: $pendingImport) { pending in
                ImportLandingHost(
                    transport: transport,
                    route: pending.route,
                    fileName: pending.fileName,
                    deviceName: mainModel.deviceName,
                    onSave: { detail in
                        mainModel.addImportedRoute(detail)
                        pendingImport = nil
                    },
                    // "Uploading saves it too" (B5): the route lands in Planned
                    // the moment the upload completes; the cover closes after F₂.
                    onUploaded: { detail in mainModel.addImportedRoute(detail) },
                    onCancel: { pendingImport = nil }
                )
            }
            .task {
                if let importAtLaunch {
                    openImport(data: importAtLaunch.data, fileName: importAtLaunch.fileName)
                }
            }
        }
        // Share-sheet / "open with OBC" delivery: iOS hands route files here
        // (registered in project.yml → CFBundleDocumentTypes). Same path as a
        // Files pick, so a Komoot share lands on E1.
        .onOpenURL { url in
            importFile(at: url)
        }
    }

    // MARK: Detail destinations (B4)

    @ViewBuilder
    private func detailScreen(for destination: MainDestination) -> some View {
        switch destination {
        case .route(let id):
            if let route = mainModel.routes.first(where: { $0.id == id }) {
                RouteDetailScreen(
                    transport: transport,
                    dressing: .planned(route),
                    // Routes saved from an import keep their parsed waypoints/
                    // profile app-side; the device can't serve them.
                    preloadedDetail: mainModel.importedDetail(for: id),
                    deviceName: mainModel.deviceName,
                    onDelete: {
                        mainModel.deleteRoute(id)
                        path.removeAll()
                    },
                    onRename: { mainModel.renameRoute(id, to: $0) }
                )
            }
        case .ride(let id):
            if let ride = mainModel.rides.first(where: { $0.id == id }) {
                RouteDetailScreen(
                    transport: transport,
                    dressing: .tracked(ride),
                    deviceName: mainModel.deviceName,
                    onRename: { mainModel.renameRide(id, to: $0) }
                )
            }
        }
    }

    // MARK: Import edge (→ E1)

    private func importFile(at url: URL) {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        guard let data = try? Data(contentsOf: url) else {
            importFailedToast = true
            return
        }
        openImport(data: data, fileName: url.lastPathComponent)
    }

    private func openImport(data: Data, fileName: String) {
        do {
            let route = try importer.importRoute(
                from: data,
                fileExtension: (fileName as NSString).pathExtension
            )
            pendingImport = PendingImport(route: route, fileName: fileName)
        } catch {
            // TODO(B6): the full H5 screen; the toast keeps failures visible
            // until the import flow owns its edge cases.
            importFailedToast = true
        }
    }
}

/// Pushed-detail routing. Carries only ids — the screens look the live summary
/// up in `MainScreenModel`, so a rename mid-stack stays consistent.
enum MainDestination: Hashable {
    case route(id: RouteID)
    case ride(id: RideID)
}

private struct PendingImport: Identifiable {
    let id = UUID()
    let route: ImportedRoute
    let fileName: String
}

/// One presented upload (B5) — carries the sheet's model, created **once** at
/// the Upload tap (built inline in the `.sheet` closure it would be rebuilt on
/// every body pass, restarting the transfer).
private struct UploadRequest: Identifiable {
    let id = UUID()
    let model: UploadSheetModel
}

/// Owns a stable `RouteDetailModel` for a pushed E2/E3 (a model created inline
/// in `navigationDestination` would be rebuilt on every body pass) — and the
/// B5 upload sheet, presented over the detail (the app never leaves the route).
private struct RouteDetailScreen: View {
    @State private var model: RouteDetailModel
    @State private var uploadRequest: UploadRequest?
    private let transport: any DeviceTransport
    private let deviceName: String
    private let onDelete: (() -> Void)?
    private let onRename: ((String) -> Void)?
    private let isRide: Bool

    init(
        transport: any DeviceTransport,
        dressing: RouteDetailModel.Dressing,
        preloadedDetail: RouteDetail? = nil,
        deviceName: String,
        onDelete: (() -> Void)? = nil,
        onRename: ((String) -> Void)? = nil
    ) {
        _model = State(initialValue: RouteDetailModel(
            transport: transport, dressing: dressing, preloadedDetail: preloadedDetail
        ))
        self.transport = transport
        self.deviceName = deviceName
        self.onDelete = onDelete
        self.onRename = onRename
        if case .tracked = dressing { isRide = true } else { isRide = false }
    }

    var body: some View {
        RouteDetailView(
            model: model,
            deviceName: deviceName,
            onUpload: {
                uploadRequest = UploadRequest(model: UploadSheetModel(
                    transport: transport,
                    blob: model.makeUploadBlob(),
                    deviceName: deviceName
                ))
            },
            onDelete: onDelete,
            onRename: onRename
        )
        .navigationTitle(isRide ? "Ride" : "Route")
        .navigationBarTitleDisplayMode(.inline)
        .sheet(item: $uploadRequest) { request in
            UploadSheetView(model: request.model)
        }
    }
}

/// Owns a stable model for the presented E1 cover, and turns Save into the
/// summary `MainScreenModel` lands in Planned. Upload presents the B5 sheet:
/// a completed upload also saves the route ("Uploading saves it too"), and
/// the cover closes once the sheet does.
private struct ImportLandingHost: View {
    @State private var model: RouteDetailModel
    @State private var uploadRequest: UploadRequest?
    @State private var uploadCompleted = false
    private let transport: any DeviceTransport
    private let deviceName: String
    private let onSave: (RouteDetail) -> Void
    private let onUploaded: (RouteDetail) -> Void
    private let onCancel: () -> Void

    init(
        transport: any DeviceTransport,
        route: ImportedRoute,
        fileName: String,
        deviceName: String,
        onSave: @escaping (RouteDetail) -> Void,
        onUploaded: @escaping (RouteDetail) -> Void,
        onCancel: @escaping () -> Void
    ) {
        _model = State(initialValue: RouteDetailModel(
            transport: transport,
            dressing: .imported(route, fileName: fileName)
        ))
        self.transport = transport
        self.deviceName = deviceName
        self.onSave = onSave
        self.onUploaded = onUploaded
        self.onCancel = onCancel
    }

    var body: some View {
        ImportLandingView(
            model: model,
            deviceName: deviceName,
            onUpload: {
                uploadRequest = UploadRequest(model: UploadSheetModel(
                    transport: transport,
                    blob: model.makeUploadBlob(),
                    deviceName: deviceName,
                    onCompleted: {
                        uploadCompleted = true
                        onUploaded(model.makeDetail())
                    }
                ))
            },
            onSave: { onSave(model.makeDetail()) },
            onCancel: onCancel
        )
        .sheet(
            item: $uploadRequest,
            // The route is already in Planned (saved on completion) — closing
            // the F₂ sheet also closes the landing. A canceled upload stays
            // on E1, still unsaved.
            onDismiss: { if uploadCompleted { onCancel() } }
        ) { request in
            UploadSheetView(model: request.model)
        }
    }
}

#if DEBUG
import OBCMock

#Preview("Bonded (main)") {
    let control = MockControl(scenario: .happyPath)
    RootView(transport: MockTransport(control: control), bondStore: MockBondStore(control: control))
}

#Preview("First run (pairing)") {
    let control = MockControl(scenario: .noDevice)
    RootView(transport: MockTransport(control: control), bondStore: MockBondStore(control: control))
}

#Preview("Import landing (E1)") {
    let control = MockControl(scenario: .happyPath)
    RootView(
        transport: MockTransport(control: control),
        bondStore: MockBondStore(control: control),
        importAtLaunch: SampleRouteFile.data().map { ($0, "sample-import.gpx") }
    )
}
#endif
