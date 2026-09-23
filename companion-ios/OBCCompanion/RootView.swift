import SwiftUI
import OBCDomain
import OBCTransport
import OBCFormats
import OBCUI

/// The app's root: the launch gate, a bond check and quiet reconnect or the pairing flow, in
/// front of the main screen, which pushes the detail screens. Holds only the seams the
/// composition root chose, plus the file-format edge that turns a picked file into an import.
/// The import flow itself is `ImportFlowModel`; this view only binds its presentation state.
struct RootView: View {
    @State private var launchModel: LaunchFlowModel
    @State private var mainModel: MainScreenModel
    @State private var importModel: ImportFlowModel
    /// The foreground-only link policy: a real background transition suspends the link, after
    /// draining any in-flight transfer; foreground re-raises it over the bonded reconnect path.
    @State private var lifecycleModel: LinkLifecycleModel
    /// Online and offline signal for the basemap previews, injected into the tree.
    @State private var reachability: ReachabilityStore
    /// Decides, on becoming active, whether a published firmware update is worth a sheet. Answers
    /// from the cache, so foregrounding is not a network request.
    @State private var updateSurfaceModel: UpdateSurfaceModel
    @State private var path: [MainDestination] = []
    @Environment(\.scenePhase) private var scenePhase

    private let transport: any DeviceTransport
    private let bondStore: any BondStore
    /// The proactive-update preferences: the auto-check toggle, the answered ledger and the
    /// last-seen device. Shared with the Settings toggle, so the switch silences what it names.
    private let updateSurface: any UpdateSurfaceStore
    /// The in-flight transfer ledger, shared by the upload sheets and the ride-sync coordinator as
    /// writers, and by the lifecycle model as the reader draining before a background disconnect.
    private let transferActivity: TransferActivity
    /// The registered import formats. One more format is one more decoder here; the picker filter
    /// and the share-sheet registration follow `supportedFileExtensions`.
    private let importer: RouteImporter
    private let rideExporter = RideExporter(encoders: [GPXRideEncoder()], defaultFileExtension: "gpx")
    /// A route file handed in at launch, which opens the import landing once the main screen is up.
    private let importAtLaunch: (data: Data, fileName: String)?
    /// A pre-staged firmware update handed in at launch, which pushes the update screen straight
    /// to its staged state, because the Files picker cannot be driven from automation.
    private let firmwareDemoAtLaunch: (data: Data, autoSend: Bool)?

    init(
        transport: any DeviceTransport,
        bondStore: any BondStore,
        library: any LibraryStore = InMemoryLibraryStore(),
        lastBikeType: LastBikeTypeStore = LastBikeTypeStore(),
        reachability: any NetworkReachability = PathMonitorReachability(),
        backgroundTasks: any BackgroundTaskRunner = UIKitBackgroundTaskRunner(),
        updateSurface: any UpdateSurfaceStore = InMemoryUpdateSurfaceStore(),
        updateNotifier: (any UpdateNotifying)? = nil,
        importAtLaunch: (data: Data, fileName: String)? = nil,
        firmwareDemoAtLaunch: (data: Data, autoSend: Bool)? = nil,
        // The sync coordinator's own timing seam, threaded so the composition root can park the
        // post-sync confirmation for an automated capture. Untouched in every ordinary run.
        syncTiming: RideSyncCoordinator.Timing = RideSyncCoordinator.Timing()
    ) {
        self.transport = transport
        self.bondStore = bondStore
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
            lastBikeType: lastBikeType,
            syncTiming: syncTiming,
            // Once per established connection, push the bond record's desired name if the device
            // config disagrees, which heals a rename whose write never landed.
            nameReconciler: DeviceNameReconciler(transport: transport, bondStore: bondStore),
            transferActivity: transferActivity
        ))
        _importModel = State(initialValue: ImportFlowModel(
            // The decode stays app-side, because OBCUI does not import OBCFormats; the flow model
            // gets a closure over it, and a narrow bond check for the framing.
            decode: { data, fileName in
                try importer.importRoute(from: data, fileExtension: (fileName as NSString).pathExtension)
            },
            library: library,
            isBonded: { bondStore.load() != nil },
            lastBikeType: lastBikeType
        ))
        _reachability = State(initialValue: ReachabilityStore(reachability))
        // The runner is the same type the background refresh runs, so the sheet and the
        // notification cannot disagree about what is worth raising.
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
                // The main screen draws its own chrome, but the title still names the pop target.
                .navigationTitle("Routes")
                .navigationDestination(for: MainDestination.self) { destination in
                    detailScreen(for: destination)
                }
            }
        }
        // Everything below hangs outside the launch gate: a share can arrive before pairing, so
        // the import cover and its alert must present over the pairing flow too.
        .fullScreenCover(item: $importModel.pendingImport) { pending in
            importLanding(for: pending)
        }
        // The share sheet can hand over anything; say what we accept.
        .alert("Couldn't read that file", isPresented: $importModel.importFailed) {
            Button("OK", role: .cancel) {}
        } message: {
            Text("OBC imports GPX and TCX route files. That one looked like something else.")
        }
        // A re-import whose name matches a saved route, such as an edited tour: update that route
        // in place, or keep both.
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
            // Remember the device's version while the link can be read, and run the launch check
            // once for this cold start; the `.active` edge below covers every return.
            updateSurfaceModel.start()
            updateSurfaceModel.appBecameActive()
            // A notice tapped from a cold launch: iOS delivers the response during startup, so
            // the flag may already be set by the time the first `.task` runs.
            if UpdateRouteRequest.shared.consume() { pushFirmwareUpdate() }
            if let importAtLaunch {
                importModel.open(data: importAtLaunch.data, fileName: importAtLaunch.fileName)
            }
            // Push the update screen, pre-staged, once the main screen is up: the demo entry the
            // Files picker cannot provide from automation.
            if firmwareDemoAtLaunch != nil, path.isEmpty {
                path = [.firmwareUpdate]
            }
        }
        // Only a real `.background` transition suspends the link; the model ignores `.inactive`
        // flickers such as the notification shade and the app switcher.
        .onChange(of: scenePhase) { _, newPhase in
            lifecycleModel.scenePhaseChanged(to: newPhase)
            // The launch check on every return to the front, and the background wake requested on
            // the way out. `.inactive` is deliberately neither.
            switch newPhase {
            case .active:
                updateSurfaceModel.appBecameActive()

            case .background: BackgroundUpdateRefresh.schedule()
            default: break
            }
        }
        // Share-sheet delivery: iOS hands route files here, the same path as a Files pick.
        .onOpenURL { url in
            Task { await importModel.openFile(at: url) }
        }
        // One shared online and offline signal for every basemap preview.
        .environment(\.obcIsOnline, reachability.isOnline)
        // Hold the screen awake while any transfer is in flight: the idle-timer touch reads the
        // same ledger the upload sheets, ride sync and firmware send claim from.
        .keepAwakeDuringTransfers(transferActivity)
        // The launch sheet, presented only when the policy says so: auto-check on, a parseable
        // running version, a fresh answer of "available", and this version not already put to the
        // rider. A swipe-down routes to `dismiss()`, because closing it is an answer.
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
        // A tapped update notice lands on the update screen. A cold-launch tap is picked up by
        // the `.task` above, a foreground one here.
        .onChange(of: UpdateRouteRequest.shared.openFirmwareUpdate) { _, wants in
            if wants, UpdateRouteRequest.shared.consume() { pushFirmwareUpdate() }
        }
    }

    // MARK: The proactive update surface

    /// Presentation binding for the launch sheet. Dismissal answers: a swipe-away says "not now".
    private var pendingUpdate: Binding<UpdateSurfaceModel.PendingUpdate?> {
        Binding(
            get: { updateSurfaceModel.pending },
            set: { if $0 == nil { updateSurfaceModel.dismiss() } }
        )
    }

    /// Push the update screen, from the sheet or from a tapped notification. Idempotent: a second
    /// copy on the stack would strand the in-flight transfer the lower one owns.
    private func pushFirmwareUpdate() {
        guard !path.contains(.firmwareUpdate) else { return }
        path.append(.firmwareUpdate)
    }

    // MARK: Import-flow presentation pieces
    //
    // Extracted from `body` for the type-checker, not for reuse: the launch gate plus ten chained
    // presentation modifiers form one expression, and each addition pushed inference time up
    // until the compiler gave up. Keep new presentation logic in helpers like these, not inline.

    /// The import cover's content: save, upload and pair-detour actions around one pending import.
    private func importLanding(for pending: PendingImport) -> some View {
        ImportLandingHost(
            transport: transport,
            activity: transferActivity,
            route: pending.route,
            fileName: pending.fileName,
            source: pending.source,
            bikeType: pending.bikeType,
            deviceName: mainModel.deviceName,
            noDevicePaired: pending.noDevicePaired,
                    // A trip is app-local, so the picker works with no device paired just the same.
            tripPickerItems: mainModel.tripPickerItems,
            replacing: pending.replacing,
                    // Replace by id only when the replaced route's link is valid for this device.
            replacingDeviceObjectID: pending.replacing.flatMap {
                mainModel.plannedDeviceObjectID(for: $0.id)
            },
            replacingProvenCRC: pending.replacing.flatMap {
                mainModel.plannedProvenCommittedCRC(for: $0.id)
            },
            onSave: { detail, tripSelection in
                mainModel.addImportedRoute(pending.record(for: detail))
                    // File into the chosen trip as its last stage; `.none` leaves it loose.
                mainModel.fileRoute(detail.summary.id, into: tripSelection)
                importModel.closeImport()
            },
                    // Uploading saves it too: the route lands in Planned the moment the upload
                    // completes, under the id the device assigned, and the cover closes after it.
                    // The model scopes the recorded link to the connected device's identity.
            onUploaded: { detail, tripSelection, objectID, crc in
                mainModel.addImportedRoute(pending.record(for: detail))
                mainModel.fileRoute(detail.summary.id, into: tripSelection)
                if let objectID {
                    mainModel.markRouteUploaded(
                        detail.summary.id, objectID: objectID, crc32: crc)
                }
            },
                    // Save first, so a pairing detour does not cost the import, then start the scan.
            onPair: { detail, tripSelection in
                mainModel.addImportedRoute(pending.record(for: detail))
                mainModel.fileRoute(detail.summary.id, into: tripSelection)
                importModel.closeImport()
                launchModel.startPairing()
            },
            onCancel: { importModel.closeImport() }
        )
    }

    /// The collision dialog's title: the imported route's name, or the file name, quoted.
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

    // MARK: Detail destinations

    @ViewBuilder
    private func detailScreen(for destination: MainDestination) -> some View {
        switch destination {
        case .route(let id):
            if let route = mainModel.routes.first(where: { $0.id == id }) {
                RouteDetailScreen(
                    transport: transport,
                    activity: transferActivity,
                    dressing: .planned(route),
                    // Routes saved from an import keep their parsed waypoints and profile
                    // app-side; the device cannot serve them.
                    preloadedDetail: mainModel.importedDetail(for: id),
                    // And their geometry, which an upload re-encodes.
                    plannedGeometry: mainModel.plannedGeometry(for: id),
                    bikeType: mainModel.plannedBikeType(for: id),
                    // And the device link, so a re-upload replaces in place and the button knows
                    // whether the copy is current.
                    deviceObjectID: mainModel.plannedDeviceObjectID(for: id),
                    provenCommittedCRC: mainModel.plannedProvenCommittedCRC(for: id),
                    deviceName: mainModel.deviceName,
                    onDelete: {
                        mainModel.deleteRoute(id)
                        path.removeAll()
                    },
                    onRename: { mainModel.renameRoute(id, to: $0) },
                    onBikeTypeChange: { mainModel.setBikeType(id, to: $0) },
                    // Reverse lands a flipped copy alongside the original and opens it.
                    onReverse: {
                        if let reversedID = mainModel.reverseRoute(id) {
                            path.append(.route(id: reversedID))
                        }
                    },
                    onUploaded: { objectID, crc in
                        if let objectID {
                            mainModel.markRouteUploaded(
                                id, objectID: objectID, crc32: crc)
                        }
                    },
                    // Add to trip on a loose route; Move to trip and Remove from trip on a filed one.
                    tripPickerItems: mainModel.tripPickerItems,
                    currentTripID: mainModel.tripContaining(id),
                    onAddToTrip: { mainModel.fileRoute(id, into: $0) },
                    onRemoveFromTrip: { mainModel.removeRouteFromTrip(id) }
                )
            }
        case .ride(let id):
            if let ride = mainModel.rides.first(where: { $0.id == id }) {
                let tracked = mainModel.ride(id)
                RouteDetailScreen(
                    transport: transport,
                    activity: transferActivity,
                    dressing: .tracked(ride),
                    bikeType: ride.bikeType,
                    // The full tracklog: the interactive map and the profile use it, never the preview.
                    ridePoints: tracked?.points ?? [],
                    deviceName: mainModel.deviceName,
                    // Phone-side only: the ride stays on the device's card and lands in Recently Deleted.
                    onDelete: {
                        mainModel.deleteRide(id)
                        path.removeAll()
                    },
                    onRename: { mainModel.renameRide(id, to: $0) },
                    onBikeTypeChange: { mainModel.setRideBikeType(id, to: $0) },
                    rideShareMenu: tracked.map(rideShareMenu(for:))
                )
            }
        case .trip(let id):
            TripDetailView(
                model: mainModel,
                tripID: id,
                // A stage opens the ordinary route detail, as a top-level route card does.
                onSelectRoute: { route in path.append(.route(id: route.id)) },
                // The trip dissolved or was deleted: pop back and drop anything pushed above it.
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
                // The same store the launch surface reads, so this toggle silences both surfaces.
                updateSurface: updateSurface,
                onDeviceRenamed: { mainModel.deviceRenamed(to: $0) },
                // Bond cleared and link dropped by the model; pop the stack and show the pairing prompt.
                onForget: {
                    path.removeAll()
                    launchModel.forgetDevice()
                },
                // Its own destination, so the host owns a stable model and an in-flight transfer
                // survives Settings body passes.
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

    /// Share the ride as GPX or an image, or save it as a route through the import landing, with
    /// the import's name-collision rule.
    private func rideShareMenu(for ride: Ride) -> RideShareMenu {
        let exporter = rideExporter
        let fileName = RideGPXFile.fileName(for: ride.summary.name)
        return RideShareMenu(
            gpx: RideGPXFile(ride: ride, encode: { try exporter.export($0).data }),
            onSaveAsRoute: ride.plannedRoute().map { route in
                { importModel.open(route: route, fileName: fileName, fileData: Data(), source: .ride(ride.summary.date), bikeType: ride.summary.bikeType) }
            }
        )
    }

    /// The hidden dev-panel entry Settings hosts: Debug-only, and only when the mock is driving.
    /// Release and forced-BLE runs pass nil, so the gesture goes nowhere.
    private var devPanelOpener: (() -> Void)? {
        #if DEBUG
        guard OBCCompanionApp.mockControl != nil else { return nil }
        return { NotificationCenter.default.post(name: .obcDeviceDidShake, object: nil) }
        #else
        return nil
        #endif
    }
}

/// Pushed-detail routing. Carries only ids, so the screens look the live summary up in
/// `MainScreenModel` and a rename mid-stack stays consistent.
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
