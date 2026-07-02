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
    @State private var importFailedAlert = false

    private let transport: any DeviceTransport
    /// Kept for the import edge: an arriving file checks the bond to pick the
    /// E1 vs H4 framing (the launch flow owns the record itself).
    private let bondStore: any BondStore
    /// The phone-side library (B1S) — the main screen reads it; the import
    /// edge writes the saved `PlannedRouteRecord`s into it through the model.
    private let library: any LibraryStore
    /// The registered import formats (B6): GPX + TCX. Adding a format = one
    /// more decoder here; the picker filter and share-sheet registration
    /// follow `supportedFileExtensions`.
    private let importer = RouteImporter(decoders: [GPXRouteDecoder(), TCXRouteDecoder()])
    /// A route file handed in at launch (`-OBCImportSample`) — opens E1 as soon
    /// as the main screen is up.
    private let importAtLaunch: (data: Data, fileName: String)?

    init(
        transport: any DeviceTransport,
        bondStore: any BondStore,
        library: any LibraryStore = InMemoryLibraryStore(),
        importAtLaunch: (data: Data, fileName: String)? = nil
    ) {
        self.transport = transport
        self.bondStore = bondStore
        self.library = library
        self.importAtLaunch = importAtLaunch
        _launchModel = State(initialValue: LaunchFlowModel(transport: transport, bondStore: bondStore))
        _mainModel = State(initialValue: MainScreenModel(transport: transport, library: library))
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
                        path.append(.settings)
                    }
                )
                // Back label for the pushed details — the main screen draws its
                // own chrome, but the title still names the pop target ("‹ Routes").
                .navigationTitle("Routes")
                .navigationDestination(for: MainDestination.self) { destination in
                    detailScreen(for: destination)
                }
            }
        }
        // Everything below hangs OUTSIDE the launch gate: a share can arrive
        // before pairing (H4) — the E1 cover and the H5 alert must present
        // over D1 just as they do over the main screen.
        .fullScreenCover(item: $pendingImport) { pending in
            ImportLandingHost(
                transport: transport,
                route: pending.route,
                fileName: pending.fileName,
                deviceName: mainModel.deviceName,
                noDevicePaired: pending.noDevicePaired,
                onSave: { detail in
                    mainModel.addImportedRoute(pending.record(for: detail))
                    pendingImport = nil
                },
                // "Uploading saves it too" (B5): the route lands in Planned
                // the moment the upload completes; the cover closes after F₂.
                onUploaded: { detail in
                    mainModel.addImportedRoute(pending.record(for: detail, uploaded: true))
                },
                // H4 "Pair a device": save first (a pairing detour must not
                // cost the import), then drop into the D2 scan.
                onPair: { detail in
                    mainModel.addImportedRoute(pending.record(for: detail))
                    pendingImport = nil
                    launchModel.startPairing()
                },
                onCancel: { pendingImport = nil }
            )
        }
        // H5 — the share sheet can hand over anything; say what we accept.
        .alert("Couldn't read that file", isPresented: $importFailedAlert) {
            Button("OK", role: .cancel) {}
        } message: {
            Text("OBC imports GPX and TCX route files. That one looked like something else.")
        }
        .task {
            if let importAtLaunch {
                openImport(data: importAtLaunch.data, fileName: importAtLaunch.fileName)
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
                    onRename: { mainModel.renameRoute(id, to: $0) },
                    // A later upload of an H4-saved route: remember the
                    // device now has a copy.
                    onUploaded: { mainModel.markRouteUploaded(id) }
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
        case .settings:
            SettingsScreen(
                transport: transport,
                bondStore: bondStore,
                onDeviceRenamed: { mainModel.deviceRenamed(to: $0) },
                // H2: bond is cleared + link dropped by the model; pop the
                // stack and hand the launch flow back to the D1 prompt.
                onForget: {
                    path.removeAll()
                    launchModel.forgetDevice()
                },
                onOpenDevPanel: devPanelOpener
            )
        }
    }

    /// The hidden dev-panel entry Settings hosts (B1P's second entry point):
    /// Debug-only, and only when the mock is driving — Release and forced-BLE
    /// runs pass `nil`, so the gesture goes nowhere.
    private var devPanelOpener: (() -> Void)? {
        #if DEBUG
        guard OBCCompanionApp.mockControl != nil else { return nil }
        return { NotificationCenter.default.post(name: .obcDeviceDidShake, object: nil) }
        #else
        return nil
        #endif
    }

    // MARK: Import edge (→ E1)

    private func importFile(at url: URL) {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        guard let data = try? Data(contentsOf: url) else {
            importFailedAlert = true
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
            pendingImport = PendingImport(
                route: route,
                fileName: fileName,
                fileData: data,
                noDevicePaired: bondStore.load() == nil
            )
        } catch {
            importFailedAlert = true
        }
    }
}

/// Pushed-detail routing. Carries only ids — the screens look the live summary
/// up in `MainScreenModel`, so a rename mid-stack stays consistent.
enum MainDestination: Hashable {
    case route(id: RouteID)
    case ride(id: RideID)
    case settings
}

/// Owns a stable `SettingsModel` for the pushed G screen (B8) — same rule as
/// the detail hosts: a model created inline in `navigationDestination` would
/// be rebuilt on every body pass.
private struct SettingsScreen: View {
    @State private var model: SettingsModel
    private let onOpenDevPanel: (() -> Void)?

    init(
        transport: any DeviceTransport,
        bondStore: any BondStore,
        onDeviceRenamed: @escaping (String) -> Void,
        onForget: @escaping () -> Void,
        onOpenDevPanel: (() -> Void)?
    ) {
        _model = State(initialValue: SettingsModel(
            transport: transport,
            bondStore: bondStore,
            onDeviceRenamed: onDeviceRenamed,
            onForget: onForget
        ))
        self.onOpenDevPanel = onOpenDevPanel
    }

    var body: some View {
        SettingsView(model: model, onOpenDevPanel: onOpenDevPanel)
    }
}

private struct PendingImport: Identifiable {
    let id = UUID()
    let route: ImportedRoute
    let fileName: String
    /// The original bytes, kept for the library record (re-parse/debugging).
    let fileData: Data
    /// Bond state at arrival — picks the E1 vs H4 framing.
    let noDevicePaired: Bool

    /// The library record (B1S) a save/upload/pair action lands: the landing's
    /// summary over the canonical parsed route + the original file.
    func record(for detail: RouteDetail, uploaded: Bool = false) -> PlannedRouteRecord {
        PlannedRouteRecord(
            summary: detail.summary,
            route: route,
            sourceFileName: fileName,
            sourceFileData: fileData,
            uploadedToDevice: uploaded
        )
    }
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
    private let onUploaded: (() -> Void)?
    private let isRide: Bool

    init(
        transport: any DeviceTransport,
        dressing: RouteDetailModel.Dressing,
        preloadedDetail: RouteDetail? = nil,
        deviceName: String,
        onDelete: (() -> Void)? = nil,
        onRename: ((String) -> Void)? = nil,
        onUploaded: (() -> Void)? = nil
    ) {
        _model = State(initialValue: RouteDetailModel(
            transport: transport, dressing: dressing, preloadedDetail: preloadedDetail
        ))
        self.transport = transport
        self.deviceName = deviceName
        self.onDelete = onDelete
        self.onRename = onRename
        self.onUploaded = onUploaded
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
                    deviceName: deviceName,
                    onCompleted: onUploaded ?? {}
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
    private let noDevicePaired: Bool
    private let onSave: (RouteDetail) -> Void
    private let onUploaded: (RouteDetail) -> Void
    private let onPair: (RouteDetail) -> Void
    private let onCancel: () -> Void

    init(
        transport: any DeviceTransport,
        route: ImportedRoute,
        fileName: String,
        deviceName: String,
        noDevicePaired: Bool,
        onSave: @escaping (RouteDetail) -> Void,
        onUploaded: @escaping (RouteDetail) -> Void,
        onPair: @escaping (RouteDetail) -> Void,
        onCancel: @escaping () -> Void
    ) {
        _model = State(initialValue: RouteDetailModel(
            transport: transport,
            dressing: .imported(route, fileName: fileName)
        ))
        self.transport = transport
        self.deviceName = deviceName
        self.noDevicePaired = noDevicePaired
        self.onSave = onSave
        self.onUploaded = onUploaded
        self.onPair = onPair
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
            onCancel: onCancel,
            noDevicePaired: noDevicePaired,
            onPair: { onPair(model.makeDetail()) }
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
