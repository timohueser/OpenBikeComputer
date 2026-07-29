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
    /// The proactive update surface (#773 U5): decides, on becoming active, whether a published
    /// firmware update is worth a sheet. Answers from U4's 6-hour cache, so foregrounding is not a
    /// network request.
    @State private var updateSurfaceModel: UpdateSurfaceModel
    @State private var path: [MainDestination] = []
    @Environment(\.scenePhase) private var scenePhase

    private let transport: any DeviceTransport
    private let bondStore: any BondStore
    /// The app-local default-retention preference (epic #638) — shared by the main
    /// model (upload seeding) and the Settings model (the Auto-delete picker), so a
    /// change in Settings seeds the next upload.
    private let retentionDefaults: any RetentionDefaultsStore
    /// The proactive-update preferences (#773 U5) — the auto-check toggle, the answered ledger and
    /// the last-seen device. Shared by the launch surface here and the Settings toggle, so the switch
    /// silences the surface it names.
    private let updateSurface: any UpdateSurfaceStore
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
        retentionDefaults: any RetentionDefaultsStore = InMemoryRetentionDefaultsStore(),
        reachability: any NetworkReachability = PathMonitorReachability(),
        backgroundTasks: any BackgroundTaskRunner = UIKitBackgroundTaskRunner(),
        updateSurface: any UpdateSurfaceStore = InMemoryUpdateSurfaceStore(),
        updateNotifier: (any UpdateNotifying)? = nil,
        importAtLaunch: (data: Data, fileName: String)? = nil,
        firmwareDemoAtLaunch: (data: Data, autoSend: Bool)? = nil
    ) {
        self.transport = transport
        self.bondStore = bondStore
        self.retentionDefaults = retentionDefaults
        self.updateSurface = updateSurface
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
            retentionDefaults: retentionDefaults,
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
        // #773 U5 — the launch surface. The runner (policy + U4's checker + this store) is the
        // *same* type the background refresh runs, so the sheet and the notification can't disagree
        // about what's worth raising.
        _updateSurfaceModel = State(initialValue: UpdateSurfaceModel(
            transport: transport,
            bondStore: bondStore,
            runner: UpdateSurfaceRunner(store: updateSurface),
            notifier: updateNotifier
        ))
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
                    onSelectTrip: { trip in
                        path.append(.trip(id: trip.id))
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
            importLanding(for: pending)
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
            collisionTitle,
            isPresented: collisionShown,
            titleVisibility: .visible,
            presenting: importModel.collision
        ) { _ in
            Button("Update the existing route") { importModel.chooseReplace() }
            Button("Add as a new route") { importModel.chooseAddAsNew() }
            Button("Cancel", role: .cancel) { importModel.cancelCollision() }
        }
        .alert(
            "Name the new route",
            isPresented: addAsNewShown,
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
            // #773 U5: remember the device's version while the link can be read, and run the launch
            // check once for this cold start (the `.active` edge below covers every return).
            updateSurfaceModel.start()
            updateSurfaceModel.appBecameActive()
            // A notice tapped from a cold launch: iOS delivers the response during startup, so the
            // flag may already be set by the time the first `.task` runs.
            if UpdateRouteRequest.shared.consume() { pushFirmwareUpdate() }
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
            // #773 U5: the launch check on every return to the front (cache-backed — see
            // `UpdateSurfaceModel`), and the background wake requested on the way out. `.inactive`
            // is deliberately neither: it's the shade/app-switcher flicker the link policy ignores
            // too.
            switch newPhase {
            case .active: updateSurfaceModel.appBecameActive()
            case .background: BackgroundUpdateRefresh.schedule()
            default: break
            }
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
        // #773 U5 — the launch sheet. Presented only when the policy says so (auto-check on, a
        // parseable running version, a fresh answer of `available`, and this version not already
        // put to the rider); a swipe-down is routed to `dismiss()` because closing it *is* an
        // answer, the same as Not now.
        .sheet(item: pendingUpdate) { update in
            UpdateAvailableSheet(
                update: update,
                onView: {
                    updateSurfaceModel.viewUpdate()
                    pushFirmwareUpdate()
                },
                onNotNow: { updateSurfaceModel.dismiss() }
            )
            .presentationDetents([.height(400)])
        }
        // A tapped update notice lands on S7 (the delegate set in `OBCCompanionApp` flips the flag;
        // a cold-launch tap is picked up by the `.task` above, a foreground one here).
        .onChange(of: UpdateRouteRequest.shared.openFirmwareUpdate) { _, wants in
            if wants, UpdateRouteRequest.shared.consume() { pushFirmwareUpdate() }
        }
    }

    // MARK: The proactive update surface (#773 U5)

    /// Presentation binding for the launch sheet. Dismissal answers — a rider who swipes it away has
    /// said "not now" just as deliberately as one who taps it.
    private var pendingUpdate: Binding<UpdateSurfaceModel.PendingUpdate?> {
        Binding(
            get: { updateSurfaceModel.pending },
            set: { if $0 == nil { updateSurfaceModel.dismiss() } }
        )
    }

    /// Push S7, from the sheet's View or from a tapped notification. Idempotent: a second tap while
    /// the screen is already on the stack must not stack a second copy (which would strand the
    /// in-flight transfer the lower one owns).
    private func pushFirmwareUpdate() {
        guard !path.contains(.firmwareUpdate) else { return }
        path.append(.firmwareUpdate)
    }

    // MARK: Import-flow presentation pieces
    //
    // Extracted from `body` — not for reuse but for the type-checker: the launch
    // gate + ten chained presentation modifiers with inline closures and
    // `Binding(get:set:)` constructions form one expression, and each addition
    // pushed inference time up until Xcode gave up ("unable to type-check this
    // expression in reasonable time", #754's `.keepAwakeDuringTransfers` was the
    // straw). Keep new presentation logic in helpers like these, not inline.

    /// The E1 import cover's content (H4): save / upload / pair-detour actions
    /// around one pending import.
    private func importLanding(for pending: PendingImport) -> some View {
        ImportLandingHost(
            transport: transport,
            activity: transferActivity,
            route: pending.route,
            fileName: pending.fileName,
            deviceName: mainModel.deviceName,
            noDevicePaired: pending.noDevicePaired,
            // The optional TR7 "Add to trip" row's picker offers the existing
            // trips (+ New trip…); a trip is app-local, so it works with no
            // device paired just the same.
            tripPickerItems: mainModel.tripPickerItems,
            replacing: pending.replacing,
            // Scope-gated (#769): replace-by-id only when the replaced route's
            // link is valid for the connected device's (serial, epoch).
            replacingDeviceObjectID: pending.replacing.flatMap {
                mainModel.plannedDeviceObjectID(for: $0.id)
            },
            replacingProvenCRC: pending.replacing.flatMap {
                mainModel.plannedProvenCommittedCRC(for: $0.id)
            },
            // Retention (epic #638 S7): a fresh import's upload sheet seeds its
            // Auto-delete row from the app default (a replace keeps the replaced
            // route's level); the capability gate hides the row on old firmware.
            uploadRetentionSeed: pending.replacing.flatMap { mainModel.plannedRetention(for: $0.id) }
                ?? mainModel.defaultRetention,
            supportsRetention: mainModel.supportsRetention,
            onSave: { detail, tripSelection in
                mainModel.addImportedRoute(pending.record(for: detail))
                // File into the chosen trip as its last stage (TR7); `.none`
                // leaves it loose (the opt-in default).
                mainModel.fileRoute(detail.summary.id, into: tripSelection)
                importModel.closeImport()
            },
            // "Uploading saves it too" (B5): the route lands in Planned
            // the moment the upload completes (recorded as on-device, up to
            // date, under the id the device assigned); the cover closes
            // after F₂. The link is recorded through `markRouteUploaded` —
            // the model scopes it to the connected device's (serial, epoch)
            // identity (#769); `record(for:)` itself never mints links.
            onUploaded: { detail, tripSelection, objectID, crc, retention in
                mainModel.addImportedRoute(pending.record(for: detail))
                mainModel.fileRoute(detail.summary.id, into: tripSelection)
                if let objectID {
                    mainModel.markRouteUploaded(
                        detail.summary.id, objectID: objectID, crc32: crc, retention: retention)
                }
            },
            // H4 "Pair a device": save first (a pairing detour must not
            // cost the import), then drop into the D2 scan.
            onPair: { detail, tripSelection in
                mainModel.addImportedRoute(pending.record(for: detail))
                mainModel.fileRoute(detail.summary.id, into: tripSelection)
                importModel.closeImport()
                launchModel.startPairing()
            },
            onCancel: { importModel.closeImport() }
        )
    }

    /// The collision dialog's title — the imported route's name (file name when
    /// the route carries none), quoted.
    private var collisionTitle: String {
        let name = importModel.collision?.pending.route.name ?? importModel.collision?.pending.fileName ?? ""
        return "\u{201C}\(name)\u{201D} is already in your library"
    }

    /// Presentation binding for the collision dialog; dismissal cancels.
    private var collisionShown: Binding<Bool> {
        Binding(
            get: { importModel.collision != nil },
            set: { if !$0 { importModel.cancelCollision() } }
        )
    }

    /// Presentation binding for the "Name the new route" prompt; dismissal cancels.
    private var addAsNewShown: Binding<Bool> {
        Binding(
            get: { importModel.addAsNewPrompt != nil },
            set: { if !$0 { importModel.cancelAddAsNew() } }
        )
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
                    provenCommittedCRC: mainModel.plannedProvenCommittedCRC(for: id),
                    deviceName: mainModel.deviceName,
                    // Retention (epic #638 S7): the desired level + the device's
                    // expiry truth for the detail row, the seed for the upload
                    // sheet, the capability gate, and the edit sink (pushes live
                    // or at the next reconcile).
                    retention: mainModel.plannedRetention(for: id),
                    deviceRetention: mainModel.plannedDeviceRetention(for: id),
                    deviceExpiresAt: mainModel.plannedDeviceExpiresAt(for: id),
                    uploadRetentionSeed: mainModel.plannedRetention(for: id) ?? mainModel.defaultRetention,
                    supportsRetention: mainModel.supportsRetention,
                    onEditRetention: { mainModel.setRouteRetention(id, $0) },
                    onDelete: {
                        mainModel.deleteRoute(id)
                        path.removeAll()
                    },
                    onRename: { mainModel.renameRoute(id, to: $0) },
                    // #503 — reverse lands a flipped copy alongside the original
                    // and opens it, so the rider sees the direction they'll ride.
                    onReverse: {
                        if let reversedID = mainModel.reverseRoute(id) {
                            path.append(.route(id: reversedID))
                        }
                    },
                    // A completed upload: record the device object id +
                    // fingerprint it landed under (the badge + in-place replace),
                    // and the rider's chosen retention (S6 pushes it post-commit).
                    onUploaded: { objectID, crc, retention in
                        if let objectID {
                            mainModel.markRouteUploaded(
                                id, objectID: objectID, crc32: crc, retention: retention)
                        }
                    },
                    // TR7 route menu (detail overflow): Add to trip… on a loose
                    // route, Move to trip… + Remove from trip on a filed one.
                    tripPickerItems: mainModel.tripPickerItems,
                    currentTripID: mainModel.tripContaining(id),
                    onAddToTrip: { mainModel.fileRoute(id, into: $0) },
                    onRemoveFromTrip: { mainModel.removeRouteFromTrip(id) }
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
        case .trip(let id):
            TripDetailView(
                model: mainModel,
                tripID: id,
                // A stage opens the ordinary route detail (E2), exactly as a
                // top-level route card does.
                onSelectRoute: { route in path.append(.route(id: route.id)) },
                // The trip dissolved or was deleted — pop back to the routes list
                // (drop the trip and anything pushed above it).
                onClose: {
                    if let index = path.firstIndex(of: .trip(id: id)) {
                        path.removeSubrange(index...)
                    }
                }
            )
        case .trash:
            RecentlyDeletedView(model: mainModel)
        case .settings:
            SettingsScreen(
                transport: transport,
                bondStore: bondStore,
                retentionDefaults: retentionDefaults,
                // #773 U5: the same store the launch surface reads, so the toggle it hosts silences
                // both proactive surfaces at once.
                updateSurface: updateSurface,
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
    case trip(id: TripID)
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
