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
    @State private var rideRenameShown = false
    @Environment(\.scenePhase) private var scenePhase

    private let transport: any DeviceTransport
    private let bondStore: any BondStore
    private let library: any LibraryStore
    private let photoLibrary: any PhotoLibrary
    /// Names places from coordinates; nil in mock runs.
    private let placeName: (@Sendable (Coordinate) async -> String?)?
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
    /// Route files handed in at launch, which open the import once the main screen is up.
    private let importAtLaunch: [(data: Data, fileName: String)]
    /// A pre-staged firmware update handed in at launch, which pushes the update screen straight
    /// to its staged state, because the Files picker cannot be driven from automation.
    private let firmwareDemoAtLaunch: (data: Data, autoSend: Bool)?

    init(
        transport: any DeviceTransport,
        bondStore: any BondStore,
        library: any LibraryStore = InMemoryLibraryStore(),
        photoLibrary: any PhotoLibrary = PhotoKitLibrary(),
        lastBikeType: LastBikeTypeStore = LastBikeTypeStore(),
        reachability: any NetworkReachability = PathMonitorReachability(),
        backgroundTasks: any BackgroundTaskRunner = UIKitBackgroundTaskRunner(),
        updateSurface: any UpdateSurfaceStore = InMemoryUpdateSurfaceStore(),
        updateNotifier: (any UpdateNotifying)? = nil,
        importAtLaunch: [(data: Data, fileName: String)] = [],
        firmwareDemoAtLaunch: (data: Data, autoSend: Bool)? = nil,
        // The sync coordinator's own timing seam, threaded so the composition root can park the
        // post-sync confirmation for an automated capture. Untouched in every ordinary run.
        syncTiming: RideSyncCoordinator.Timing = RideSyncCoordinator.Timing(),
        placeName: (@Sendable (Coordinate) async -> String?)? = nil,
        stopSearch: (any StopSearch)? = nil,
        legRouter: (any LegRouter)? = nil
    ) {
        self.transport = transport
        self.bondStore = bondStore
        self.library = library
        self.photoLibrary = photoLibrary
        self.placeName = placeName
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
            transferActivity: transferActivity,
            placeName: placeName,
            stopSearch: stopSearch,
            legRouter: legRouter
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
                    onImportFile: { urls in
                        Task { await importModel.openFiles(at: urls) }
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
        .fullScreenCover(item: $importModel.pendingJoin) { join in
            joinSheet(for: join)
        }
        // The share sheet can hand over anything; say what we accept.
        .alert("Couldn't read that file", isPresented: $importModel.importFailed) {
            Button("OK", role: .cancel) {}
        } message: {
            Text("OBC imports GPX and TCX route files. That one looked like something else.")
        }
        // A trip change that did not go as asked says so, in one line, where the rider made it.
        .alert(
            mainModel.tripNotice ?? "",
            isPresented: Binding(
                get: { mainModel.tripNotice != nil },
                set: { if !$0 { mainModel.tripNotice = nil } })
        ) {
            Button("OK", role: .cancel) {}
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
        .obcRenameSheet(
            "Name the new route",
            isPresented: addAsNewShown,
            name: importModel.newRouteName,
            message: "A route with this name is already in your library — pick a different one.",
            saveTitle: "Add",
            canSave: importModel.isValidNewRouteName,
            onSave: importModel.confirmNewName
        )
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
            if !importAtLaunch.isEmpty {
                importModel.open(files: importAtLaunch)
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
            // A share of several files arrives one URL at a time; the model batches them.
            importModel.receive(url)
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
            route: pending.route,
            fileName: pending.fileName,
            source: pending.source,
            bikeType: pending.bikeType,
            deviceName: mainModel.deviceName,
            noDevicePaired: pending.noDevicePaired,
            // A trip is app-local, so the trip rows work with no device paired just the same.
            trips: mainModel.tripPickerItems,
            replacing: pending.replacing,
            onSave: { detail, tripSelection in
                mainModel.addImportedRoute(pending.record(for: detail))
                // A trip choice moves the route into the trip and opens the trip page.
                // A new trip from one file opens in the day editor's split mode.
                if let tripID = mainModel.fileRoute(detail.summary.id, into: tripSelection) {
                    path = [.trip(id: tripID)]
                    if case .new = tripSelection { path.append(.dayEditor(id: tripID, isSplitMode: true)) }
                }
                importModel.closeImport()
            },
            // Save first, so a pairing detour does not cost the import, then start the scan.
            onPair: { detail in
                mainModel.addImportedRoute(pending.record(for: detail))
                importModel.closeImport()
                launchModel.startPairing()
            },
            onCancel: { importModel.closeImport() }
        )
    }

    /// Several files at once: one trip with a day per file, or each file as a route.
    private func joinSheet(for join: PendingJoin) -> some View {
        TripJoinSheet(
            files: join.files.enumerated().map { index, file in
                TripJoinSheet.File(id: index, fileName: file.fileName, points: file.route.points)
            },
            onMakeTrip: { ordered in
                // A file too short to be a day stays a route, and the rider is told.
                let short = ordered.filter { !Trip.isDay($0.points) }
                for file in short { saveAsRoute(join.files[file.id]) }
                mainModel.noteTooShort(short.map(\.fileName))
                let tripID = mainModel.createTrip(
                    name: "New trip", files: ordered.map(\.points),
                    dayNames: ordered.map { ($0.fileName as NSString).deletingPathExtension },
                    waypoints: ordered.map { join.files[$0.id].route.waypoints })
                importModel.closeJoin()
                if let tripID { path = [.trip(id: tripID)] }
            },
            onNotNow: {
                join.files.forEach(saveAsRoute)
                importModel.closeJoin()
            }
        )
    }

    /// Save one file of a join as a route, with the summary the import landing would make.
    private func saveAsRoute(_ file: PendingImport) {
        let detail = RouteDetailModel(
            transport: transport, dressing: .imported(file.route, fileName: file.fileName),
            bikeType: file.bikeType
        ).makeDetail()
        mainModel.addImportedRoute(file.record(for: detail))
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
                    sourceFileName: mainModel.plannedSourceFileName(for: id),
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
                    // The route moves into the trip, so the trip page replaces the route page.
                    tripPickerItems: mainModel.tripPickerItems,
                    onAddToTrip: { selection in
                        guard let tripID = mainModel.fileRoute(id, into: selection) else { return }
                        path.removeLast()
                        path.append(.trip(id: tripID))
                    }
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
                    rides: mainModel.rides,
                    photos: (library, photoLibrary),
                    placeName: placeName,
                    deviceName: mainModel.deviceName,
                    // Phone-side only: the ride stays on the device's card and lands in Recently Deleted.
                    onDelete: {
                        mainModel.deleteRide(id)
                        path.removeAll()
                    },
                    onRenameTap: { rideRenameShown = true },
                    onBikeTypeChange: { mainModel.setRideBikeType(id, to: $0) },
                    rideShareMenu: tracked.map(rideShareMenu(for:)),
                    rideEditMenu: tracked.map { rideEditMenu(for: $0) },
                    quietRows: AnyView(RideMergeSuggestion(
                        load: { await mainModel.mergeSuggestion(for: id) },
                        onMerge: { mainModel.mergeRideWithNext(id) },
                        onDismiss: { mainModel.dismissMergeSuggestion(for: id) }
                    ))
                )
                // An edit, new points, or a new device revision of the ride build the screen's
                // models again: they hold the ride they were built with. A revision can change the
                // summary alone, such as its zone limits, with every sample the same.
                .id(tracked.map { [Double(mainModel.rideEditCount), Double($0.points.count),
                                   $0.points.first?.timestamp.timeIntervalSince1970 ?? 0,
                                   $0.points.last?.timestamp.timeIntervalSince1970 ?? 0,
                                   Double(ride.source?.revision ?? 0)] })
                // A rename builds the screen again too, so the title shows the new name.
                .id(ride.name)
                // Above both rebuilds, so an edit that lands while the sheet is open cannot close it.
                .obcRenameSheet(
                    "Rename ride",
                    isPresented: $rideRenameShown,
                    name: ride.name,
                    canSave: { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
                ) {
                    mainModel.renameRide(id, to: $0.trimmingCharacters(in: .whitespacesAndNewlines))
                }
            }
        case .trip(let id):
            TripDetailView(
                model: mainModel,
                tripID: id,
                // The trip was deleted: pop back and drop anything pushed above it.
                onClose: {
                    if let index = path.firstIndex(of: .trip(id: id)) {
                        path.removeSubrange(index...)
                    }
                },
                onOpenRide: { path.append(.ride(id: $0)) },
                onEvenOut: { offer in Task { await mainModel.evenOutDays(id, from: offer.fixedBefore) } },
                onEditDays: { path.append(.dayEditor(id: id, isSplitMode: false)) },
                onOpenDay: { path.append(.tripDay(id: id, day: $0)) },
                encodeGPX: { GPXTripEncoder.encode($0) },
                uploadTiming: TripUploadModel.Timing(
                    doneAutoDismiss: OBCCompanionApp.launchUploadTiming().doneAutoDismiss)
            )
        case .dayEditor(let id, let isSplitMode):
            DayEditorHost(make: { mainModel.dayEditor(id, isSplitMode: isSplitMode) }) {
                if let index = path.lastIndex(of: .dayEditor(id: id, isSplitMode: isSplitMode)) {
                    path.removeSubrange(index...)
                }
            }
        case .tripDay(let id, let day):
            if let trip = mainModel.trip(id), let route = mainModel.tripDays(id).first(where: { $0.day == day }) {
                RouteDetailScreen(
                    transport: transport,
                    dressing: .tripDay(route.summary(tripID: id)),
                    preloadedDetail: route.detail(tripID: id),
                    // The full-resolution cut, for the interactive map.
                    plannedGeometry: ImportedRoute(points: route.points),
                    bikeType: trip.bikeType,
                    deviceName: mainModel.deviceName
                )
            }
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
    private func rideShareMenu(for ride: Ride) -> ShareMenu {
        let exporter = rideExporter
        let fileName = GPXFile.fileName(for: ride.summary.name)
        return ShareMenu(
            gpx: GPXFile(name: ride.summary.name) { try exporter.export(ride).data },
            image: ShareCardContent(ride: ride)
        )
        .saveAsRoute(ride.plannedRoute().map { route in
            { importModel.open(route: route, fileName: fileName, fileData: Data(), source: .ride(ride.summary.date), bikeType: ride.summary.bikeType) }
        })
    }

    /// Edit ride and Revert to original.
    private func rideEditMenu(for ride: Ride) -> RideEditMenu {
        let id = ride.id
        return RideEditMenu(
            ride: ride,
            nextRide: mainModel.nextRide(after: id),
            isEdited: mainModel.isEditedRide(id),
            onEdit: { edit in
                switch edit {
                case .trim(let range): mainModel.trimRide(id, to: range)
                case .mergeWithNext: mainModel.mergeRideWithNext(id)
                }
            },
            onRevert: { mainModel.revertRide(id) }
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

/// Owns the day editor's draft for the screen's life. The destination body runs on every pass
/// of the root, so the model is made once, on appear, not in the body.
private struct DayEditorHost: View {
    let make: () -> TripDayEditorModel?
    let onClose: () -> Void
    @State private var model: TripDayEditorModel?

    var body: some View {
        if let model {
            TripDayEditorView(model: model, onClose: onClose)
        } else {
            OBCTheme.page.ignoresSafeArea().onAppear { model = make() }
        }
    }
}

/// Pushed-detail routing. Carries only ids, so the screens look the live summary up in
/// `MainScreenModel` and a rename mid-stack stays consistent.
enum MainDestination: Hashable {
    case route(id: RouteID)
    case trip(id: TripID)
    /// The trip's day editor, in split mode when one file just became the trip.
    case dayEditor(id: TripID, isSplitMode: Bool)
    case tripDay(id: TripID, day: Int)
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
        importAtLaunch: SampleRouteFile.files()
    )
}
#endif
