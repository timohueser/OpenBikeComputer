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
    /// The route-menu picker, planned dressing only: the detail overflow's Add or Move to trip
    /// presents the shared picker sheet.
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
    /// Trip filing, planned only: the existing trips, this route's current trip, where nil means
    /// loose and offers Add while non-nil offers Move and Remove, and the two edits. A nil
    /// `onAddToTrip` suppresses the overflow entirely.
    private let tripPickerItems: [TripPickerItem]
    private let currentTripID: TripID?
    private let onAddToTrip: ((TripSelection) -> Void)?
    private let onRemoveFromTrip: (() -> Void)?
    /// The share button, rides with a tracklog only.
    private let rideShareMenu: RideShareMenu?

    init(
        transport: any DeviceTransport,
        activity: TransferActivity? = nil,
        dressing: RouteDetailModel.Dressing,
        preloadedDetail: RouteDetail? = nil,
        plannedGeometry: ImportedRoute? = nil,
        bikeType: BikeType = .road,
        rideGeometry: [Coordinate]? = nil,
        deviceObjectID: DeviceObjectID? = nil,
        provenCommittedCRC: UInt32? = nil,
        deviceName: String,
        onDelete: (() -> Void)? = nil,
        onRename: ((String) -> Void)? = nil,
        onBikeTypeChange: ((BikeType) -> Void)? = nil,
        onReverse: (() -> Void)? = nil,
        onUploaded: ((DeviceObjectID?, UInt32) -> Void)? = nil,
        tripPickerItems: [TripPickerItem] = [],
        currentTripID: TripID? = nil,
        onAddToTrip: ((TripSelection) -> Void)? = nil,
        onRemoveFromTrip: (() -> Void)? = nil,
        rideShareMenu: RideShareMenu? = nil
    ) {
        _model = State(initialValue: RouteDetailModel(
            transport: transport, dressing: dressing, bikeType: bikeType,
            preloadedDetail: preloadedDetail, plannedGeometry: plannedGeometry,
            deviceObjectID: deviceObjectID, provenCommittedCRC: provenCommittedCRC,
            rideGeometry: rideGeometry
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
        self.currentTripID = currentTripID
        self.onAddToTrip = onAddToTrip
        self.onRemoveFromTrip = onRemoveFromTrip
        self.rideShareMenu = rideShareMenu
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
            onBikeTypeChange: onBikeTypeChange
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
                ToolbarItem(placement: .primaryAction) { rideShareMenu }
            }
        }
        .sheet(item: $uploadRequest) { request in
            UploadSheetView(model: request.model)
        }
        .sheet(isPresented: $tripPickerShown) {
            TripPickerSheet(
                title: currentTripID == nil ? "Add to trip" : "Move to trip",
                trips: tripPickerItems,
                currentTripID: currentTripID,
                onPick: { onAddToTrip?($0) }
            )
        }
    }

    /// The detail overflow's trip menu: Add to trip for a loose route, or Move to trip and Remove
    /// from trip for one already filed.
    private func tripMenu(onAddToTrip: @escaping (TripSelection) -> Void) -> some View {
        Menu {
            if currentTripID == nil {
                Button {
                    tripPickerShown = true
                } label: {
                    Label("Add to trip…", systemImage: "folder.badge.plus")
                }
                .accessibilityIdentifier("detail.addToTrip")
            } else {
                Button {
                    tripPickerShown = true
                } label: {
                    Label("Move to trip…", systemImage: "folder")
                }
                .accessibilityIdentifier("detail.moveToTrip")
                Button(role: .destructive) {
                    onRemoveFromTrip?()
                } label: {
                    Label("Remove from trip", systemImage: "minus.circle")
                }
                .accessibilityIdentifier("detail.removeFromTrip")
            }
        } label: {
            Image(systemName: "ellipsis.circle")
        }
        .accessibilityIdentifier("detail.overflow")
    }
}

/// Owns a stable model for the presented import cover, and turns Save into the summary the main
/// model lands in Planned. Upload presents the upload sheet: a completed upload also saves the
/// route, and the cover closes once the sheet does.
struct ImportLandingHost: View {
    @State private var model: RouteDetailModel
    @State private var uploadRequest: UploadRequest?
    @State private var uploadCompleted = false
    /// The optional "Add to trip" choice for this import, opt-in and default none, and the shared
    /// picker's presentation.
    @State private var tripSelection: TripSelection = .none
    @State private var tripPickerShown = false
    private let transport: any DeviceTransport
    /// The in-flight ledger the upload sheet claims a token from.
    private let activity: TransferActivity?
    private let deviceName: String
    private let noDevicePaired: Bool
    /// Existing trips for the import row's picker. Empty means no trips yet, and the row still
    /// offers a new one.
    private let tripPickerItems: [TripPickerItem]
    private let onSave: (RouteDetail, TripSelection) -> Void
    private let onUploaded: (RouteDetail, TripSelection, DeviceObjectID?, UInt32) -> Void
    private let onPair: (RouteDetail, TripSelection) -> Void
    private let onCancel: () -> Void

    init(
        transport: any DeviceTransport,
        activity: TransferActivity? = nil,
        route: ImportedRoute,
        fileName: String,
        source: ImportSource,
        bikeType: BikeType,
        deviceName: String,
        noDevicePaired: Bool,
        tripPickerItems: [TripPickerItem] = [],
        // When this import replaces an existing route, the landing reuses its id and device link,
        // so a save or upload updates that route in place instead of adding a duplicate. The old
        // fingerprint is what makes the button read "Update".
        replacing: PlannedRouteRecord? = nil,
        // The replace-by-id target for an upload from this landing. The caller derives it through
        // the scope-gated helper, so a link minted on another device or era can never aim the
        // upload at the wrong object.
        replacingDeviceObjectID: DeviceObjectID? = nil,
        // The proven-held CRC of the route being replaced: the button reads "up to date" only on
        // the same proof the list badge uses, never on a stale link.
        replacingProvenCRC: UInt32? = nil,
        onSave: @escaping (RouteDetail, TripSelection) -> Void,
        onUploaded: @escaping (RouteDetail, TripSelection, DeviceObjectID?, UInt32) -> Void,
        onPair: @escaping (RouteDetail, TripSelection) -> Void,
        onCancel: @escaping () -> Void
    ) {
        _model = State(initialValue: RouteDetailModel(
            transport: transport,
            dressing: .imported(route, fileName: fileName, source: source),
            bikeType: bikeType,
            deviceObjectID: replacingDeviceObjectID,
            provenCommittedCRC: replacingProvenCRC,
            importedRouteID: replacing?.id
        ))
        self.transport = transport
        self.activity = activity
        self.deviceName = deviceName
        self.noDevicePaired = noDevicePaired
        self.tripPickerItems = tripPickerItems
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
                    // Normally the sheet self-dismisses; parked under the hold flag, so a capture
                    // cannot lose the sheet.
                    timing: OBCCompanionApp.launchUploadTiming(),
                    activity: activity,
                    onCompleted: { [model] objectID, crc in
                        uploadCompleted = true
                        if let objectID { model.recordUploaded(objectID: objectID, crc32: crc) }
                        onUploaded(model.makeDetail(), tripSelection, objectID, crc)
                    }
                ))
            },
            onSave: { onSave(model.makeDetail(), tripSelection) },
            onCancel: onCancel,
            noDevicePaired: noDevicePaired,
            onPair: { onPair(model.makeDetail(), tripSelection) },
            importAccessory: AnyView(tripRow)
        )
        .sheet(
            item: $uploadRequest,
            // The route is already in Planned, saved on completion, so closing the confirm sheet
            // also closes the landing. A cancelled upload stays on the landing, still unsaved.
            onDismiss: { if uploadCompleted { onCancel() } }
        ) { request in
            UploadSheetView(model: request.model)
        }
        .sheet(isPresented: $tripPickerShown) {
            TripPickerSheet(
                title: "Add to trip",
                trips: tripPickerItems,
                allowsNone: true,
                onPick: { tripSelection = $0 }
            )
        }
    }

    /// The optional "Add to trip" row: opt-in, default None. It opens the shared picker and shows
    /// the current choice.
    private var tripRow: some View {
        OBCDisclosureRow(
            systemImage: "folder.badge.plus",
            label: "Add to trip",
            value: tripSelectionLabel,
            accessibilityID: "import.addToTrip",
            action: { tripPickerShown = true }
        )
        .padding(.bottom, 2)
    }

    /// The current import trip choice as a row value: None, an existing trip's name, or the new
    /// trip's name.
    private var tripSelectionLabel: String {
        switch tripSelection {
        case .none: "None"
        case .existing(let id): tripPickerItems.first { $0.id == id }?.name ?? "Trip"
        case .new(let name): name
        }
    }
}
