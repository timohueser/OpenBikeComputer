import SwiftUI
import OBCDomain
import OBCTransport
import OBCFormats
import OBCUI

/// The app's root: the launch gate (bond check → quiet reconnect, or the
/// pairing flow) in front of the main screen, which pushes the detail
/// screens. Holds only the seams the composition root chose —
/// `any DeviceTransport` + `any BondStore` — plus the file-format edge
/// (`RouteImporter`) that turns a picked file into the import landing.
struct RootView: View {
    @State private var launchModel: LaunchFlowModel
    @State private var mainModel: MainScreenModel
    /// Online/offline signal for the MapKit basemap previews, injected into
    /// the whole tree as `\.obcIsOnline`.
    @State private var reachability: ReachabilityStore
    @State private var path: [MainDestination] = []
    @State private var pendingImport: PendingImport?
    @State private var importCollision: ImportCollision?
    @State private var importFailedAlert = false
    /// "Add as a new route" chosen from the collision dialog — holds the import
    /// while the distinct-name prompt is up.
    @State private var addAsNewPrompt: PendingImport?
    @State private var newRouteName = ""

    private let transport: any DeviceTransport
    /// Kept for the import edge: an arriving file checks the bond to pick the
    /// saved-vs-no-device framing (the launch flow owns the record itself).
    private let bondStore: any BondStore
    /// The phone-side library — the main screen reads it; the import edge
    /// writes the saved `PlannedRouteRecord`s into it through the model.
    private let library: any LibraryStore
    /// The registered import formats: GPX + TCX. Adding a format = one more
    /// decoder here; the picker filter and share-sheet registration follow
    /// `supportedFileExtensions`.
    private let importer = RouteImporter(decoders: [GPXRouteDecoder(), TCXRouteDecoder()])
    /// A route file handed in at launch (`-OBCImportSample`) — opens the
    /// import landing as soon as the main screen is up.
    private let importAtLaunch: (data: Data, fileName: String)?

    init(
        transport: any DeviceTransport,
        bondStore: any BondStore,
        library: any LibraryStore = InMemoryLibraryStore(),
        reachability: any NetworkReachability = PathMonitorReachability(),
        importAtLaunch: (data: Data, fileName: String)? = nil
    ) {
        self.transport = transport
        self.bondStore = bondStore
        self.library = library
        self.importAtLaunch = importAtLaunch
        _launchModel = State(initialValue: LaunchFlowModel(transport: transport, bondStore: bondStore))
        _mainModel = State(initialValue: MainScreenModel(transport: transport, library: library))
        _reachability = State(initialValue: ReachabilityStore(reachability))
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
        // before pairing — the import cover and the read-failure alert must
        // present over the pairing flow just as they do over the main screen.
        .fullScreenCover(item: $pendingImport) { pending in
            ImportLandingHost(
                transport: transport,
                route: pending.route,
                fileName: pending.fileName,
                deviceName: mainModel.deviceName,
                noDevicePaired: pending.noDevicePaired,
                replacing: pending.replacing,
                onSave: { detail in
                    mainModel.addImportedRoute(pending.record(for: detail))
                    pendingImport = nil
                },
                // "Uploading saves it too": the route lands in Planned the
                // moment the upload completes (recorded as on-device, up to
                // date, under the id the device assigned); the cover closes
                // once the upload sheet does.
                onUploaded: { detail, objectID, crc in
                    mainModel.addImportedRoute(pending.record(for: detail, deviceObjectID: objectID, uploadedCRC32: crc))
                },
                // "Pair a device": save first (a pairing detour must not cost
                // the import), then drop into the scan.
                onPair: { detail in
                    mainModel.addImportedRoute(pending.record(for: detail))
                    pendingImport = nil
                    launchModel.startPairing()
                },
                onCancel: { pendingImport = nil }
            )
        }
        // The share sheet can hand over anything; say what we accept.
        .alert("Couldn't read that file", isPresented: $importFailedAlert) {
            Button("OK", role: .cancel) {}
        } message: {
            Text("OBC imports GPX and TCX route files. That one looked like something else.")
        }
        // A re-import whose name matches a saved route (e.g. an edited Komoot
        // tour): update that route in place, or keep both.
        .confirmationDialog(
            "\u{201C}\(importCollision?.pending.route.name ?? importCollision?.pending.fileName ?? "")\u{201D} is already in your library",
            isPresented: Binding(get: { importCollision != nil }, set: { if !$0 { importCollision = nil } }),
            titleVisibility: .visible,
            presenting: importCollision
        ) { collision in
            Button("Update the existing route") {
                pendingImport = collision.pending.replacing(collision.existing)
                importCollision = nil
            }
            Button("Add as a new route") {
                // Two routes under one name would be indistinguishable (and the
                // next import's collision check keys on the name) — a distinct
                // name is required before the landing opens.
                newRouteName = collision.pending.route.name ?? collision.pending.fileName
                addAsNewPrompt = collision.pending
                importCollision = nil
            }
            Button("Cancel", role: .cancel) { importCollision = nil }
        }
        .alert(
            "Name the new route",
            isPresented: Binding(get: { addAsNewPrompt != nil }, set: { if !$0 { addAsNewPrompt = nil } }),
            presenting: addAsNewPrompt
        ) { pending in
            TextField("Name", text: $newRouteName)
            Button("Cancel", role: .cancel) { addAsNewPrompt = nil }
            Button("Add") {
                pendingImport = pending.renamed(to: newRouteName.trimmingCharacters(in: .whitespacesAndNewlines))
                addAsNewPrompt = nil
            }
            .disabled(!isNewRouteNameValid)
        } message: { _ in
            Text("A route with this name is already in your library — pick a different one.")
        }
        .task {
            reachability.start()
            if let importAtLaunch {
                openImport(data: importAtLaunch.data, fileName: importAtLaunch.fileName)
            }
        }
        // Share-sheet / "open with OBC" delivery: iOS hands route files here
        // (registered in project.yml → CFBundleDocumentTypes). Same path as a
        // Files pick, so a Komoot share lands on the import landing.
        .onOpenURL { url in
            importFile(at: url)
        }
        // One shared online/offline signal for every basemap preview — the
        // import cover + pushed details inherit it through the presentation.
        .environment(\.obcIsOnline, reachability.isOnline)
    }

    // MARK: Detail destinations

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
                    // …and their geometry, which an upload re-encodes to OBCR.
                    plannedGeometry: mainModel.plannedGeometry(for: id),
                    // …and the device link (object id + committed fingerprint),
                    // so a re-upload replaces in place and the button knows
                    // whether the copy is current.
                    deviceObjectID: mainModel.plannedDeviceObjectID(for: id),
                    uploadedCRC32: mainModel.plannedUploadedCRC32(for: id),
                    deviceName: mainModel.deviceName,
                    onDelete: {
                        mainModel.deleteRoute(id)
                        path.removeAll()
                    },
                    onRename: { mainModel.renameRoute(id, to: $0) },
                    // A completed upload: record the device object id +
                    // fingerprint it landed under (the badge + in-place replace).
                    onUploaded: { objectID, crc in
                        if let objectID { mainModel.markRouteUploaded(id, objectID: objectID, crc32: crc) }
                    }
                )
            }
        case .ride(let id):
            if let ride = mainModel.rides.first(where: { $0.id == id }) {
                RouteDetailScreen(
                    transport: transport,
                    dressing: .tracked(ride),
                    // The full tracklog — the interactive map draws this,
                    // never the ride card's downsampled preview.
                    rideGeometry: mainModel.rideGeometry(for: id),
                    deviceName: mainModel.deviceName,
                    // Phone-side only — the ride stays on the device's card.
                    onDelete: {
                        mainModel.deleteRide(id)
                        path.removeAll()
                    },
                    onRename: { mainModel.renameRide(id, to: $0) }
                )
            }
        case .settings:
            SettingsScreen(
                transport: transport,
                bondStore: bondStore,
                onDeviceRenamed: { mainModel.deviceRenamed(to: $0) },
                // Bond is cleared + link dropped by the model; pop the stack
                // and hand the launch flow back to the pair-intro prompt.
                onForget: {
                    path.removeAll()
                    launchModel.forgetDevice()
                },
                onOpenDevPanel: devPanelOpener
            )
        }
    }

    /// The hidden dev-panel entry Settings hosts: Debug-only, and only when
    /// the mock is driving — Release and forced-BLE runs pass `nil`, so the
    /// gesture goes nowhere.
    private var devPanelOpener: (() -> Void)? {
        #if DEBUG
        guard OBCCompanionApp.mockControl != nil else { return nil }
        return { NotificationCenter.default.post(name: .obcDeviceDidShake, object: nil) }
        #else
        return nil
        #endif
    }

    // MARK: Import edge

    /// The saved planned route whose name matches, case-insensitively. Reads
    /// the **library store directly** — a share can arrive while the launch
    /// gate is still connecting, before the main screen (and its in-memory
    /// mirror) ever started; the store is always current, every save writes
    /// through it.
    private func plannedRoute(named name: String) -> PlannedRouteRecord? {
        let target = name.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return library.plannedRoutes().first { $0.summary.name.lowercased() == target }
    }

    /// Whether the "Add as a new route" prompt's current name can be accepted:
    /// non-empty and unlike every saved route's (the collision check keys on
    /// names, so a duplicate would just re-collide).
    private var isNewRouteNameValid: Bool {
        let trimmed = newRouteName.trimmingCharacters(in: .whitespacesAndNewlines)
        return !trimmed.isEmpty && plannedRoute(named: trimmed) == nil
    }

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
            let pending = PendingImport(
                route: route,
                fileName: fileName,
                fileData: data,
                noDevicePaired: bondStore.load() == nil
            )
            // A route by this name already saved → offer update-in-place vs new.
            if let existing = plannedRoute(named: route.name ?? fileName) {
                importCollision = ImportCollision(pending: pending, existing: existing)
            } else {
                pendingImport = pending
            }
        } catch {
            importFailedAlert = true
        }
    }
}

/// A just-imported route whose name matches one already in the library — the
/// data behind the update-or-add confirmation dialog.
private struct ImportCollision: Identifiable {
    let id = UUID()
    let pending: PendingImport
    let existing: PlannedRouteRecord
}

/// Pushed-detail routing. Carries only ids — the screens look the live summary
/// up in `MainScreenModel`, so a rename mid-stack stays consistent.
enum MainDestination: Hashable {
    case route(id: RouteID)
    case ride(id: RideID)
    case settings
}

/// Owns a stable `SettingsModel` for the pushed settings screen — same rule
/// as the detail hosts: a model created inline in `navigationDestination`
/// would be rebuilt on every body pass.
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
    var route: ImportedRoute
    let fileName: String
    /// The original bytes, kept for the library record (re-parse/debugging).
    let fileData: Data
    /// Bond state at arrival — picks the saved-vs-no-device framing.
    let noDevicePaired: Bool
    /// The existing route this import replaces (name-collision → Replace), or
    /// `nil` for a fresh import. Its id + device object id carry through.
    var replacing: PlannedRouteRecord? = nil

    /// A copy pinned to replace `record` (chosen from the collision dialog).
    func replacing(_ record: PlannedRouteRecord) -> PendingImport {
        var copy = self
        copy.replacing = record
        return copy
    }

    /// A copy under a fresh name (the "Add as a new route" prompt) — a plain
    /// new import, not a replace.
    func renamed(to newName: String) -> PendingImport {
        var copy = self
        copy.route.name = newName
        copy.replacing = nil
        return copy
    }

    /// The library record a save/upload/pair action lands: the landing's
    /// summary over the canonical parsed route + the original file. `deviceObjectID`
    /// + `uploadedCRC32` are what an upload just committed; absent that, a
    /// replace keeps the route it's replacing on the device — under its old
    /// fingerprint, so the badge honestly reads **out of date** until the next
    /// push updates the copy.
    func record(for detail: RouteDetail, deviceObjectID: UInt16? = nil, uploadedCRC32: UInt32? = nil) -> PlannedRouteRecord {
        PlannedRouteRecord(
            summary: detail.summary,
            route: route,
            sourceFileName: fileName,
            sourceFileData: fileData,
            deviceObjectID: deviceObjectID ?? replacing?.deviceObjectID,
            uploadedCRC32: uploadedCRC32 ?? replacing?.uploadedCRC32
        )
    }
}

/// One presented upload — carries the sheet's model, created **once** at the
/// Upload tap (built inline in the `.sheet` closure it would be rebuilt on
/// every body pass, restarting the transfer).
private struct UploadRequest: Identifiable {
    let id = UUID()
    let model: UploadSheetModel
}

/// Owns a stable `RouteDetailModel` for a pushed detail screen (a model
/// created inline in `navigationDestination` would be rebuilt on every body
/// pass) — and the upload sheet, presented over the detail (the app never
/// leaves the route).
private struct RouteDetailScreen: View {
    @State private var model: RouteDetailModel
    @State private var uploadRequest: UploadRequest?
    private let transport: any DeviceTransport
    private let deviceName: String
    private let onDelete: (() -> Void)?
    private let onRename: ((String) -> Void)?
    private let onUploaded: ((UInt16?, UInt32) -> Void)?
    private let isRide: Bool

    init(
        transport: any DeviceTransport,
        dressing: RouteDetailModel.Dressing,
        preloadedDetail: RouteDetail? = nil,
        plannedGeometry: ImportedRoute? = nil,
        rideGeometry: [Coordinate]? = nil,
        deviceObjectID: UInt16? = nil,
        uploadedCRC32: UInt32? = nil,
        deviceName: String,
        onDelete: (() -> Void)? = nil,
        onRename: ((String) -> Void)? = nil,
        onUploaded: ((UInt16?, UInt32) -> Void)? = nil
    ) {
        _model = State(initialValue: RouteDetailModel(
            transport: transport, dressing: dressing,
            preloadedDetail: preloadedDetail, plannedGeometry: plannedGeometry,
            deviceObjectID: deviceObjectID, uploadedCRC32: uploadedCRC32,
            rideGeometry: rideGeometry
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
                    onCompleted: { [model] objectID, crc in
                        // Pin the committed id + fingerprint on the live model
                        // too — a second Upload on this same screen must
                        // replace, never duplicate.
                        if let objectID { model.recordUploaded(objectID: objectID, crc32: crc) }
                        onUploaded?(objectID, crc)
                    }
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

/// Owns a stable model for the presented import cover, and turns Save into
/// the summary `MainScreenModel` lands in Planned. Upload presents the
/// upload sheet: a completed upload also saves the route ("Uploading saves
/// it too"), and the cover closes once the sheet does.
private struct ImportLandingHost: View {
    @State private var model: RouteDetailModel
    @State private var uploadRequest: UploadRequest?
    @State private var uploadCompleted = false
    private let transport: any DeviceTransport
    private let deviceName: String
    private let noDevicePaired: Bool
    private let onSave: (RouteDetail) -> Void
    private let onUploaded: (RouteDetail, UInt16?, UInt32) -> Void
    private let onPair: (RouteDetail) -> Void
    private let onCancel: () -> Void

    init(
        transport: any DeviceTransport,
        route: ImportedRoute,
        fileName: String,
        deviceName: String,
        noDevicePaired: Bool,
        // When this import replaces an existing route (name-collision → Replace),
        // the landing reuses its id + device link so a save/upload updates
        // that route in place instead of adding a duplicate (the old fingerprint
        // makes Upload read "Update on …").
        replacing: PlannedRouteRecord? = nil,
        onSave: @escaping (RouteDetail) -> Void,
        onUploaded: @escaping (RouteDetail, UInt16?, UInt32) -> Void,
        onPair: @escaping (RouteDetail) -> Void,
        onCancel: @escaping () -> Void
    ) {
        _model = State(initialValue: RouteDetailModel(
            transport: transport,
            dressing: .imported(route, fileName: fileName),
            deviceObjectID: replacing?.deviceObjectID,
            uploadedCRC32: replacing?.uploadedCRC32,
            importedRouteID: replacing?.id
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
                    onCompleted: { [model] objectID, crc in
                        uploadCompleted = true
                        if let objectID { model.recordUploaded(objectID: objectID, crc32: crc) }
                        onUploaded(model.makeDetail(), objectID, crc)
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
            // the sheet also closes the landing. A canceled upload stays on
            // the landing, still unsaved.
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
