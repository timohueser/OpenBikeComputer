import SwiftUI
import OBCDomain
import OBCTransport
import OBCUI

/// The pushed/presented screen hosts `RootView` composes: each owns a stable
/// model for its screen (a model created inline in `navigationDestination` or
/// a presentation closure would be rebuilt on every body pass). App-target on
/// purpose — they wire OBCUI screens to the composition root's seams.

/// Owns a stable `SettingsModel` for the pushed G screen (B8) — same rule as
/// the detail hosts: a model created inline in `navigationDestination` would
/// be rebuilt on every body pass.
struct SettingsScreen: View {
    @State private var model: SettingsModel
    private let onOpenFirmwareUpdate: () -> Void
    private let onOpenDevPanel: (() -> Void)?

    init(
        transport: any DeviceTransport,
        bondStore: any BondStore,
        retentionDefaults: any RetentionDefaultsStore,
        onDeviceRenamed: @escaping (String) -> Void,
        onForget: @escaping () -> Void,
        onOpenFirmwareUpdate: @escaping () -> Void,
        onOpenDevPanel: (() -> Void)?
    ) {
        _model = State(initialValue: SettingsModel(
            transport: transport,
            bondStore: bondStore,
            retentionDefaults: retentionDefaults,
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

/// Owns a stable `FirmwareUpdateModel` for the pushed S7 screen — same rule as
/// the other hosts (a model built inline in `navigationDestination` would be
/// rebuilt on every body pass, dropping an in-flight transfer). `deviceName` is
/// passed through so the plain copy can name the device.
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
            activity: activity, prestage: prestage, autoSend: autoSend
        ))
    }

    var body: some View {
        FirmwareUpdateView(model: model)
    }
}

/// One presented upload (B5) — carries the sheet's model, created **once** at
/// the Upload tap (built inline in the `.sheet` closure it would be rebuilt on
/// every body pass, restarting the transfer).
struct UploadRequest: Identifiable {
    let id = UUID()
    let model: UploadSheetModel
}

/// Owns a stable `RouteDetailModel` for a pushed E2/E3 (a model created inline
/// in `navigationDestination` would be rebuilt on every body pass) — and the
/// B5 upload sheet, presented over the detail (the app never leaves the route).
struct RouteDetailScreen: View {
    @State private var model: RouteDetailModel
    @State private var uploadRequest: UploadRequest?
    /// TR7 route-menu picker (planned dressing only): the detail overflow's
    /// Add/Move to trip presents the shared `TripPickerSheet`.
    @State private var tripPickerShown = false
    private let transport: any DeviceTransport
    /// The #459 in-flight ledger the upload sheet claims a token from — `nil`
    /// in previews that don't exercise the lifecycle.
    private let activity: TransferActivity?
    private let deviceName: String
    private let onDelete: (() -> Void)?
    private let onRename: ((String) -> Void)?
    private let onUploaded: ((DeviceObjectID?, UInt32, Retention) -> Void)?
    /// Retention (epic #638 S7): the level the upload sheet seeds from, whether the
    /// device is capable (hides the row/skips the confirm), and the detail-edit sink.
    private let uploadRetentionSeed: Retention
    private let supportsRetention: Bool
    private let onEditRetention: ((Retention) -> Void)?
    private let isRide: Bool
    /// TR7 trip filing (planned only): the existing trips, this route's current
    /// trip (nil = loose → Add; non-nil → Move + Remove), and the two edits.
    /// `onAddToTrip == nil` suppresses the overflow entirely (rides / imports).
    private let tripPickerItems: [TripPickerItem]
    private let currentTripID: TripID?
    private let onAddToTrip: ((TripSelection) -> Void)?
    private let onRemoveFromTrip: (() -> Void)?

    init(
        transport: any DeviceTransport,
        activity: TransferActivity? = nil,
        dressing: RouteDetailModel.Dressing,
        preloadedDetail: RouteDetail? = nil,
        plannedGeometry: ImportedRoute? = nil,
        rideGeometry: [Coordinate]? = nil,
        deviceObjectID: DeviceObjectID? = nil,
        provenCommittedCRC: UInt32? = nil,
        deviceName: String,
        retention: Retention? = nil,
        deviceRetention: Retention? = nil,
        deviceExpiresAt: Date? = nil,
        uploadRetentionSeed: Retention = .appDefault,
        supportsRetention: Bool = false,
        onEditRetention: ((Retention) -> Void)? = nil,
        onDelete: (() -> Void)? = nil,
        onRename: ((String) -> Void)? = nil,
        onUploaded: ((DeviceObjectID?, UInt32, Retention) -> Void)? = nil,
        tripPickerItems: [TripPickerItem] = [],
        currentTripID: TripID? = nil,
        onAddToTrip: ((TripSelection) -> Void)? = nil,
        onRemoveFromTrip: (() -> Void)? = nil
    ) {
        _model = State(initialValue: RouteDetailModel(
            transport: transport, dressing: dressing,
            preloadedDetail: preloadedDetail, plannedGeometry: plannedGeometry,
            deviceObjectID: deviceObjectID, provenCommittedCRC: provenCommittedCRC,
            retention: retention, deviceRetention: deviceRetention,
            deviceExpiresAt: deviceExpiresAt,
            supportsRetention: supportsRetention, onEditRetention: onEditRetention,
            rideGeometry: rideGeometry
        ))
        self.transport = transport
        self.activity = activity
        self.deviceName = deviceName
        self.uploadRetentionSeed = uploadRetentionSeed
        self.supportsRetention = supportsRetention
        self.onEditRetention = onEditRetention
        self.onDelete = onDelete
        self.onRename = onRename
        self.onUploaded = onUploaded
        self.tripPickerItems = tripPickerItems
        self.currentTripID = currentTripID
        self.onAddToTrip = onAddToTrip
        self.onRemoveFromTrip = onRemoveFromTrip
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
                    retention: uploadRetentionSeed,
                    supportsRetention: supportsRetention,
                    activity: activity,
                    onCompleted: { [model] objectID, crc, retention in
                        // Pin the committed id + fingerprint on the live model
                        // too — a second Upload on this same screen must
                        // replace, never duplicate.
                        if let objectID { model.recordUploaded(objectID: objectID, crc32: crc) }
                        onUploaded?(objectID, crc, retention)
                    }
                ))
            },
            onDelete: onDelete,
            onRename: onRename
        )
        .navigationTitle(isRide ? "Ride" : "Route")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if let onAddToTrip {
                ToolbarItem(placement: .primaryAction) {
                    tripMenu(onAddToTrip: onAddToTrip)
                }
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

    /// The detail overflow's trip menu (TR7): Add to trip… for a loose route, or
    /// Move to trip… + Remove from trip for one already filed.
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

/// Owns a stable model for the presented E1 cover, and turns Save into the
/// summary `MainScreenModel` lands in Planned. Upload presents the B5 sheet:
/// a completed upload also saves the route ("Uploading saves it too"), and
/// the cover closes once the sheet does.
struct ImportLandingHost: View {
    @State private var model: RouteDetailModel
    @State private var uploadRequest: UploadRequest?
    @State private var uploadCompleted = false
    /// TR7: the optional "Add to trip" choice for this import (opt-in, default
    /// none) and the shared picker's presentation.
    @State private var tripSelection: TripSelection = .none
    @State private var tripPickerShown = false
    private let transport: any DeviceTransport
    /// The #459 in-flight ledger the upload sheet claims a token from.
    private let activity: TransferActivity?
    private let deviceName: String
    private let noDevicePaired: Bool
    /// Existing trips for the TR7 import row's picker (empty = no trips yet, so
    /// the row still offers New trip…).
    private let tripPickerItems: [TripPickerItem]
    /// Retention (epic #638 S7): the level a fresh upload's Auto-delete row seeds
    /// from, and whether the device honours it.
    private let uploadRetentionSeed: Retention
    private let supportsRetention: Bool
    private let onSave: (RouteDetail, TripSelection) -> Void
    private let onUploaded: (RouteDetail, TripSelection, DeviceObjectID?, UInt32, Retention) -> Void
    private let onPair: (RouteDetail, TripSelection) -> Void
    private let onCancel: () -> Void

    init(
        transport: any DeviceTransport,
        activity: TransferActivity? = nil,
        route: ImportedRoute,
        fileName: String,
        deviceName: String,
        noDevicePaired: Bool,
        tripPickerItems: [TripPickerItem] = [],
        // When this import replaces an existing route (name-collision → Replace),
        // the landing reuses its id + device link so a save/upload updates
        // that route in place instead of adding a duplicate (the old fingerprint
        // makes Upload read "Update on …").
        replacing: PlannedRouteRecord? = nil,
        // The replace-by-id target for an upload from this landing — the
        // caller derives it through the scope-gated helper (#769:
        // `MainScreenModel.plannedDeviceObjectID(for:)`), so a link minted on
        // another device / era can never aim the upload at the wrong object.
        replacingDeviceObjectID: DeviceObjectID? = nil,
        // The proven-held CRC of the route being replaced (#770), derived
        // through `MainScreenModel.plannedProvenCommittedCRC(for:)` — the button
        // reads "up to date" only on the same proof the list badge uses, never
        // on a stale link.
        replacingProvenCRC: UInt32? = nil,
        uploadRetentionSeed: Retention = .appDefault,
        supportsRetention: Bool = false,
        onSave: @escaping (RouteDetail, TripSelection) -> Void,
        onUploaded: @escaping (RouteDetail, TripSelection, DeviceObjectID?, UInt32, Retention) -> Void,
        onPair: @escaping (RouteDetail, TripSelection) -> Void,
        onCancel: @escaping () -> Void
    ) {
        _model = State(initialValue: RouteDetailModel(
            transport: transport,
            dressing: .imported(route, fileName: fileName),
            deviceObjectID: replacingDeviceObjectID,
            provenCommittedCRC: replacingProvenCRC,
            importedRouteID: replacing?.id
        ))
        self.transport = transport
        self.activity = activity
        self.deviceName = deviceName
        self.noDevicePaired = noDevicePaired
        self.tripPickerItems = tripPickerItems
        self.uploadRetentionSeed = uploadRetentionSeed
        self.supportsRetention = supportsRetention
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
                    retention: uploadRetentionSeed,
                    supportsRetention: supportsRetention,
                    activity: activity,
                    onCompleted: { [model] objectID, crc, retention in
                        uploadCompleted = true
                        if let objectID { model.recordUploaded(objectID: objectID, crc32: crc) }
                        onUploaded(model.makeDetail(), tripSelection, objectID, crc, retention)
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
            // The route is already in Planned (saved on completion) — closing
            // the F₂ sheet also closes the landing. A canceled upload stays
            // on E1, still unsaved.
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

    /// The optional "Add to trip" row (TR7): opt-in, default None; opens the
    /// shared picker and shows the current choice.
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

    /// The current import trip choice as a row value: None, an existing trip's
    /// name, or the new trip's name.
    private var tripSelectionLabel: String {
        switch tripSelection {
        case .none: "None"
        case .existing(let id): tripPickerItems.first { $0.id == id }?.name ?? "Trip"
        case .new(let name): name
        }
    }
}
