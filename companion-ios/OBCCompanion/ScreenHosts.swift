import SwiftUI
import OBCDomain
import OBCTransport
import OBCUI

/// The pushed and presented screen hosts `RootView` composes. Each owns a stable model for its
/// screen, because a model created inline in a destination or presentation closure would be
/// rebuilt on every body pass. App-target on purpose: they wire OBCUI screens to the composition
/// root's seams.

/// Owns a stable `SettingsModel` for the pushed screen.
struct SettingsScreen: View {
    @State private var model: SettingsModel
    private let onOpenFirmwareUpdate: () -> Void

    private let onOpenDevPanel: (() -> Void)?

    init(
        transport: any DeviceTransport,
        bondStore: any BondStore,
        updateSurface: any UpdateSurfaceStore,
        onDeviceRenamed: @escaping (String) -> Void,
        onForget: @escaping () -> Void,
        onOpenFirmwareUpdate: @escaping () -> Void,

        onOpenDevPanel: (() -> Void)?
    ) {
        _model = State(initialValue: SettingsModel(
            transport: transport,
            bondStore: bondStore,
            updateSurface: updateSurface,
            onDeviceRenamed: onDeviceRenamed,
            onForget: onForget
        ))
        self.onOpenFirmwareUpdate = onOpenFirmwareUpdate

        self.onOpenDevPanel = onOpenDevPanel
    }

    var body: some View {
        SettingsView(
            model: model,
            onOpenFirmwareUpdate: onOpenFirmwareUpdate,

            onOpenDevPanel: onOpenDevPanel
        )
    }
}

/// Owns a stable `FirmwareUpdateModel` for the pushed update screen: a model built inline would
/// be rebuilt on every body pass and drop an in-flight transfer. `deviceName` is passed through so
/// the plain copy can name the device.
struct FirmwareUpdateScreen: View {
    @State private var model: FirmwareUpdateModel

    init(
        transport: any DeviceTransport,
        deviceName: String,
        activity: TransferActivity? = nil,
        prestage: Data? = nil,
        autoSend: Bool = false
    ) {
        _model = State(initialValue: FirmwareUpdateModel(
            transport: transport, deviceName: deviceName,
            activity: activity,
            // The published-release check. The composition root is where the concrete network and
            // defaults seams are picked, exactly as it picks the transport; the model itself knows
            // only the protocol.
            updateChecker: UpdateChecker(),
            prestage: prestage, autoSend: autoSend
        ))
    }

    var body: some View {
        FirmwareUpdateView(model: model)
    }
}

/// One presented upload: it carries the sheet's model, created once at the Upload tap. Built
/// inline in the `.sheet` closure it would be rebuilt on every body pass, restarting the transfer.
struct UploadRequest: Identifiable {
    let id = UUID()
    let model: UploadSheetModel
}

/// Owns a stable `RouteDetailModel` for a pushed detail, and the upload sheet presented over it,
/// so the app never leaves the route.
struct RouteDetailScreen: View {
    @State private var model: RouteDetailModel
    @State private var uploadRequest: UploadRequest?
    /// The route-menu picker, planned dressing only: the detail overflow's Add to trip presents
    /// the shared picker sheet.
    @State private var tripPickerShown = false
    private let transport: any DeviceTransport
    /// The in-flight ledger the upload sheet claims a token from. Nil in previews.
    private let activity: TransferActivity?
    private let deviceName: String
    private let onDelete: (() -> Void)?
    private let onRename: ((String) -> Void)?
    private let onBikeTypeChange: ((BikeType) -> Void)?
    /// Reverse the route, planned dressing only: it creates the flipped copy and navigates to it.
    /// Nil on rides and imports.
    private let onReverse: (() -> Void)?
    private let onUploaded: ((DeviceObjectID?, UInt32) -> Void)?
    private let isRide: Bool
    /// Add to trip, planned only: the route becomes a day of the picked trip. A nil `onAddToTrip`
    /// suppresses the overflow entirely.
    private let tripPickerItems: [TripPickerItem]
    private let onAddToTrip: ((TripSelection) -> Void)?
    /// The share button, rides with a tracklog only.
    private let rideShareMenu: RideShareMenu?
    /// A tracked ride's photos.
    @State private var photos: RidePhotosModel?
    /// A tracked ride's day note.
    @State private var dayNote: DayNoteModel?
    /// The ⋯ menu with Edit ride and Revert to original, rides with a tracklog only.
    private let rideEditMenu: RideEditMenu?
    /// The quiet rows under a ride's stats line.
    private let quietRows: AnyView?

    init(
        transport: any DeviceTransport,
        activity: TransferActivity? = nil,
        dressing: RouteDetailModel.Dressing,
        preloadedDetail: RouteDetail? = nil,
        plannedGeometry: ImportedRoute? = nil,
        bikeType: BikeType = .road,
        ridePoints: [RidePoint] = [],
        rides: [RideSummary] = [],
        photos: (library: any LibraryStore, photoLibrary: any PhotoLibrary)? = nil,
        placeName: (@Sendable (Coordinate) async -> String?)? = nil,
        deviceObjectID: DeviceObjectID? = nil,
        provenCommittedCRC: UInt32? = nil,
        deviceName: String,
        onDelete: (() -> Void)? = nil,
        onRename: ((String) -> Void)? = nil,
        onBikeTypeChange: ((BikeType) -> Void)? = nil,
        onReverse: (() -> Void)? = nil,
        onUploaded: ((DeviceObjectID?, UInt32) -> Void)? = nil,
        tripPickerItems: [TripPickerItem] = [],
        onAddToTrip: ((TripSelection) -> Void)? = nil,
        rideShareMenu: RideShareMenu? = nil,
        rideEditMenu: RideEditMenu? = nil,
        quietRows: AnyView? = nil
    ) {
        _model = State(initialValue: RouteDetailModel(
            transport: transport, dressing: dressing, bikeType: bikeType,
            preloadedDetail: preloadedDetail, plannedGeometry: plannedGeometry,
            deviceObjectID: deviceObjectID, provenCommittedCRC: provenCommittedCRC,
            ridePoints: ridePoints, rides: rides
        ))
        self.transport = transport
        self.activity = activity
        self.deviceName = deviceName
        self.onDelete = onDelete
        self.onRename = onRename
        self.onBikeTypeChange = onBikeTypeChange
        self.onReverse = onReverse
        self.onUploaded = onUploaded
        self.tripPickerItems = tripPickerItems
        self.onAddToTrip = onAddToTrip
        self.rideShareMenu = rideShareMenu
        self.rideEditMenu = rideEditMenu
        self.quietRows = quietRows
        if case .tracked(let ride) = dressing {
            isRide = true
            if let photos {
                _photos = State(initialValue: RidePhotosModel(
                    rideID: ride.id, points: ridePoints, library: photos.library, photoLibrary: photos.photoLibrary
                ))
                _dayNote = State(initialValue: DayNoteModel(
                    ride: ride, points: ridePoints, library: photos.library, placeName: placeName
                ))
            }
        } else {
            isRide = false
        }
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
                    // Normally the sheet self-dismisses; parked under the hold flag, so a capture
                    // cannot lose the sheet.
                    timing: OBCCompanionApp.launchUploadTiming(),
                    activity: activity,
                    onCompleted: { [model] objectID, crc in
                        // Pin the committed id and fingerprint on the live model too: a second
                        // Upload on this same screen must replace, never duplicate.
                        if let objectID { model.recordUploaded(objectID: objectID, crc32: crc) }
                        onUploaded?(objectID, crc)
                    }
                ))
            },
            onDelete: onDelete,
            onRename: onRename,
            onReverse: onReverse,
            onBikeTypeChange: onBikeTypeChange,
            photos: photos,
            dayNote: dayNote,
            quietRows: quietRows
        )
        .navigationTitle(isRide ? "Ride" : "Route")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if let onAddToTrip {
                ToolbarItem(placement: .primaryAction) {
                    tripMenu(onAddToTrip: onAddToTrip)
                }
            }
            if let rideShareMenu {
                ToolbarItem(placement: .primaryAction) { rideShareMenu.photos(from: photos) }
            }
            if let rideEditMenu {
                ToolbarItem(placement: .primaryAction) { rideEditMenu }
            }
        }
        .sheet(item: $uploadRequest) { request in
            UploadSheetView(model: request.model)
        }
        .sheet(isPresented: $tripPickerShown) {
            TripPickerSheet(
                title: "Add to trip",
                trips: tripPickerItems,
                onPick: { onAddToTrip?($0) }
            )
        }
    }

    /// The detail overflow's trip menu.
    private func tripMenu(onAddToTrip: @escaping (TripSelection) -> Void) -> some View {
        Menu {
            Button {
                tripPickerShown = true
            } label: {
                Label("Add to trip…", systemImage: "folder.badge.plus")
            }
            .accessibilityIdentifier("detail.addToTrip")
        } label: {
            Image(systemName: "ellipsis.circle")
        }
        .accessibilityIdentifier("detail.overflow")
    }
}

/// Owns a stable model for the presented import cover. The three rows land the route: as a new
/// route, as the last day of a trip, or as the first day of a new trip.
struct ImportLandingHost: View {
    @State private var model: RouteDetailModel
    @State private var tripPickerShown = false
    private let deviceName: String
    private let noDevicePaired: Bool
    /// The trips the route can join, most recently edited first.
    private let trips: [TripPickerItem]
    private let onSave: (RouteDetail, TripSelection) -> Void
    private let onPair: (RouteDetail) -> Void
    private let onCancel: () -> Void

    init(
        transport: any DeviceTransport,
        route: ImportedRoute,
        fileName: String,
        source: ImportSource,
        bikeType: BikeType,
        deviceName: String,
        noDevicePaired: Bool,
        trips: [TripPickerItem] = [],
        // When this import replaces an existing route, the landing reuses its id, so New route
        // updates that route in place instead of adding a duplicate.
        replacing: PlannedRouteRecord? = nil,
        onSave: @escaping (RouteDetail, TripSelection) -> Void,
        onPair: @escaping (RouteDetail) -> Void,
        onCancel: @escaping () -> Void
    ) {
        _model = State(initialValue: RouteDetailModel(
            transport: transport,
            dressing: .imported(route, fileName: fileName, source: source),
            bikeType: bikeType,
            importedRouteID: replacing?.id
        ))
        self.deviceName = deviceName
        self.noDevicePaired = noDevicePaired
        self.trips = trips
        self.onSave = onSave
        self.onPair = onPair
        self.onCancel = onCancel
    }

    var body: some View {
        ImportLandingView(
            model: model,
            deviceName: deviceName,
            onCancel: onCancel,
            noDevicePaired: noDevicePaired,
            onPair: { onPair(model.makeDetail()) },
            importAccessory: AnyView(rows)
        )
        .sheet(isPresented: $tripPickerShown) {
            TripPickerSheet(title: "Add to trip", trips: trips, onPick: save)
        }
    }

    /// New route, Add to ‹trip› (or Add to trip with a picker when there are several), Start a
    /// trip.
    private var rows: some View {
        OBCGroupedSection {
            OBCListRow(label: "New route", showsChevron: true) { save(.none) }
                .accessibilityIdentifier("import.newRoute")
            if trips.count == 1, let trip = trips.first {
                OBCListRow(label: "Add to \(trip.name)", detail: "Becomes Day \(trip.dayCount + 1)", showsChevron: true) {
                    save(.existing(trip.id))
                }
                .accessibilityIdentifier("import.addToTrip")
            } else if trips.count > 1 {
                OBCListRow(label: "Add to trip", showsChevron: true) { tripPickerShown = true }
                    .accessibilityIdentifier("import.addToTrip")
            }
            OBCListRow(label: "Start a trip", showsChevron: true, showsDivider: false) {
                save(.new(model.name))
            }
            .accessibilityIdentifier("import.startTrip")
        }
    }

    private func save(_ selection: TripSelection) {
        onSave(model.makeDetail(), selection)
    }
}
