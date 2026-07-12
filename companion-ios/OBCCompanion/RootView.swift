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
///
/// The import flow itself (replace-vs-new, rename validation, the H5 failure)
/// is `ImportFlowModel` in OBCUI, where it runs under `swift test`; this view
/// only binds its cover/dialog/alert state and hands it the decode closure.
struct RootView: View {
    @State private var launchModel: LaunchFlowModel
    @State private var mainModel: MainScreenModel
    @State private var importModel: ImportFlowModel
    /// The foreground-only link policy (#459): a real background transition
    /// suspends the link (after draining any in-flight transfer under the
    /// system grace window); foreground re-raises it via the bonded
    /// silent-reconnect path. Fed raw `scenePhase` changes below.
    @State private var lifecycleModel: LinkLifecycleModel
    /// Online/offline signal for the MapKit basemap previews (#294), injected
    /// into the whole tree as `\.obcIsOnline`.
    @State private var reachability: ReachabilityStore
    @State private var path: [MainDestination] = []
    @Environment(\.scenePhase) private var scenePhase

    private let transport: any DeviceTransport
    private let bondStore: any BondStore
    /// The in-flight transfer ledger (#459) — shared by the upload sheets and
    /// the ride-sync coordinator (the writers) and the lifecycle model (the
    /// reader draining before a background disconnect).
    private let transferActivity: TransferActivity
    /// The registered import formats (B6): GPX + TCX. Adding a format = one
    /// more decoder here; the picker filter and share-sheet registration
    /// follow `supportedFileExtensions`.
    private let importer: RouteImporter
    /// A route file handed in at launch (`-OBCImportSample`) — opens E1 as soon
    /// as the main screen is up.
    private let importAtLaunch: (data: Data, fileName: String)?
    /// A pre-staged firmware update handed in at launch (`-OBCFirmwareDemo`) —
    /// pushes the S7 screen straight to its staged state (the Files picker can't
    /// be driven from automation), optionally auto-sending. `nil` in normal runs.
    private let firmwareDemoAtLaunch: (data: Data, autoSend: Bool)?

    init(
        transport: any DeviceTransport,
        bondStore: any BondStore,
        library: any LibraryStore = InMemoryLibraryStore(),
        reachability: any NetworkReachability = PathMonitorReachability(),
        backgroundTasks: any BackgroundTaskRunner = UIKitBackgroundTaskRunner(),
        importAtLaunch: (data: Data, fileName: String)? = nil,
        firmwareDemoAtLaunch: (data: Data, autoSend: Bool)? = nil
    ) {
        self.transport = transport
        self.bondStore = bondStore
        self.importAtLaunch = importAtLaunch
        self.firmwareDemoAtLaunch = firmwareDemoAtLaunch
        let importer = RouteImporter(decoders: [GPXRouteDecoder(), TCXRouteDecoder()])
        self.importer = importer
        let transferActivity = TransferActivity()
        self.transferActivity = transferActivity
        _launchModel = State(initialValue: LaunchFlowModel(transport: transport, bondStore: bondStore))
        _lifecycleModel = State(initialValue: LinkLifecycleModel(
            transport: transport, activity: transferActivity, backgroundTasks: backgroundTasks
        ))
        _mainModel = State(initialValue: MainScreenModel(
            transport: transport, library: library,
            // The rename self-heal (#361): once per established connection,
            // push the bond record's desired name if the device config
            // disagrees (a rename whose write never landed).
            nameReconciler: DeviceNameReconciler(transport: transport, bondStore: bondStore),
            transferActivity: transferActivity
        ))
        _importModel = State(initialValue: ImportFlowModel(
            // The decode stays app-side (formats at the edges — OBCUI doesn't
            // import OBCFormats); the flow model gets a closure over it, and a
            // narrow bond check for the E1 vs H4 framing.
            decode: { data, fileName in
                try importer.importRoute(from: data, fileExtension: (fileName as NSString).pathExtension)
            },
            library: library,
            isBonded: { bondStore.load() != nil }
        ))
        _reachability = State(initialValue: ReachabilityStore(reachability))
    }

    var body: some View {
        LaunchFlowView(model: launchModel) {
            NavigationStack(path: $path) {
                MainScreenView(
                    model: mainModel,
                    importFileExtensions: importer.supportedFileExtensions,
                    onImportFile: { url in
                        Task { await importModel.openFile(at: url) }
                    },
                    onSelectRoute: { route in
                        path.append(.route(id: route.id))
                    },
                    onSelectRide: { ride in
                        path.append(.ride(id: ride.id))
                    },
                    onSettings: {
                        path.append(.settings)
                    },
                    onOpenTrash: {
                        path.append(.trash)
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
        .fullScreenCover(item: $importModel.pendingImport) { pending in
            ImportLandingHost(
                transport: transport,
                activity: transferActivity,
                route: pending.route,
                fileName: pending.fileName,
                deviceName: mainModel.deviceName,
                noDevicePaired: pending.noDevicePaired,
                replacing: pending.replacing,
                onSave: { detail in
                    mainModel.addImportedRoute(pending.record(for: detail))
                    importModel.closeImport()
                },
                // "Uploading saves it too" (B5): the route lands in Planned
                // the moment the upload completes (recorded as on-device, up to
                // date, under the id the device assigned); the cover closes
                // after F₂.
                onUploaded: { detail, objectID, crc in
                    mainModel.addImportedRoute(pending.record(for: detail, deviceObjectID: objectID, uploadedCRC32: crc))
                },
                // H4 "Pair a device": save first (a pairing detour must not
                // cost the import), then drop into the D2 scan.
                onPair: { detail in
                    mainModel.addImportedRoute(pending.record(for: detail))
                    importModel.closeImport()
                    launchModel.startPairing()
                },
                onCancel: { importModel.closeImport() }
            )
        }
        // H5 — the share sheet can hand over anything; say what we accept.
        .alert("Couldn't read that file", isPresented: $importModel.importFailed) {
            Button("OK", role: .cancel) {}
        } message: {
            Text("OBC imports GPX and TCX route files. That one looked like something else.")
        }
        // A re-import whose name matches a saved route (e.g. an edited Komoot
        // tour): update that route in place, or keep both.
        .confirmationDialog(
            "\u{201C}\(importModel.collision?.pending.route.name ?? importModel.collision?.pending.fileName ?? "")\u{201D} is already in your library",
            isPresented: Binding(
                get: { importModel.collision != nil },
                set: { if !$0 { importModel.cancelCollision() } }
            ),
            titleVisibility: .visible,
            presenting: importModel.collision
        ) { _ in
            Button("Update the existing route") { importModel.chooseReplace() }
            Button("Add as a new route") { importModel.chooseAddAsNew() }
            Button("Cancel", role: .cancel) { importModel.cancelCollision() }
        }
        .alert(
            "Name the new route",
            isPresented: Binding(
                get: { importModel.addAsNewPrompt != nil },
                set: { if !$0 { importModel.cancelAddAsNew() } }
            ),
            presenting: importModel.addAsNewPrompt
        ) { _ in
            TextField("Name", text: $importModel.newRouteName)
            Button("Cancel", role: .cancel) { importModel.cancelAddAsNew() }
            Button("Add") { importModel.confirmNewName() }
                .disabled(!importModel.isNewRouteNameValid)
        } message: { _ in
            Text("A route with this name is already in your library — pick a different one.")
        }
        .task {
            lifecycleModel.start()
            reachability.start()
            if let importAtLaunch {
                importModel.open(data: importAtLaunch.data, fileName: importAtLaunch.fileName)
            }
            // The `-OBCFirmwareDemo` hook: push the S7 screen (pre-staged) once
            // the main screen is up — the demo/screenshot entry the Files picker
            // can't provide from automation.
            if firmwareDemoAtLaunch != nil, path.isEmpty {
                path = [.firmwareUpdate]
            }
        }
        // The foreground-only link (#459): only a real `.background` transition
        // suspends (the model ignores `.inactive` flickers — notification
        // shade, app switcher); `.active` re-raises via the bonded
        // silent-reconnect path.
        .onChange(of: scenePhase) { _, newPhase in
            lifecycleModel.scenePhaseChanged(to: newPhase)
        }
        // Share-sheet / "open with OBC" delivery: iOS hands route files here
        // (registered in project.yml → CFBundleDocumentTypes). Same path as a
        // Files pick, so a Komoot share lands on E1.
        .onOpenURL { url in
            Task { await importModel.openFile(at: url) }
        }
        // One shared online/offline signal for every basemap preview (#294) —
        // the E1 cover + pushed details inherit it through the presentation.
        .environment(\.obcIsOnline, reachability.isOnline)
        // Hold the screen awake while any transfer is in flight (#754) — the
        // idle-timer touch reads the same ledger the upload sheets, ride sync,
        // and firmware send claim from. UIKit stays at the composition root.
        .keepAwakeDuringTransfers(transferActivity)
    }

    // MARK: Detail destinations (B4)

    @ViewBuilder
    private func detailScreen(for destination: MainDestination) -> some View {
        switch destination {
        case .route(let id):
            if let route = mainModel.routes.first(where: { $0.id == id }) {
                RouteDetailScreen(
                    transport: transport,
                    activity: transferActivity,
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
                    activity: transferActivity,
                    dressing: .tracked(ride),
                    // The full tracklog (#294) — the interactive map draws this,
                    // never the ride card's downsampled preview.
                    rideGeometry: mainModel.rideGeometry(for: id),
                    deviceName: mainModel.deviceName,
                    // Phone-side only — the ride stays on the device's card;
                    // app-side it lands in Recently Deleted (#292), recoverable.
                    onDelete: {
                        mainModel.deleteRide(id)
                        path.removeAll()
                    },
                    onRename: { mainModel.renameRide(id, to: $0) }
                )
            }
        case .trash:
            RecentlyDeletedView(model: mainModel)
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
                // S7: push the firmware-update screen (its own destination so
                // the host owns a stable model — an in-flight transfer survives
                // Settings body passes).
                onOpenFirmwareUpdate: { path.append(.firmwareUpdate) },
                onOpenDevPanel: devPanelOpener
            )
        case .firmwareUpdate:
            FirmwareUpdateScreen(
                transport: transport,
                deviceName: mainModel.deviceName,
                activity: transferActivity,
                prestage: firmwareDemoAtLaunch?.data,
                autoSend: firmwareDemoAtLaunch?.autoSend ?? false
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
}

/// Pushed-detail routing. Carries only ids — the screens look the live summary
/// up in `MainScreenModel`, so a rename mid-stack stays consistent.
enum MainDestination: Hashable {
    case route(id: RouteID)
    case ride(id: RideID)
    case trash
    case settings
    case firmwareUpdate
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
