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
        onDeviceRenamed: @escaping (String) -> Void,
        onForget: @escaping () -> Void,
        onOpenFirmwareUpdate: @escaping () -> Void,
        onOpenDevPanel: (() -> Void)?
    ) {
        _model = State(initialValue: SettingsModel(
            transport: transport,
            bondStore: bondStore,
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
    private let transport: any DeviceTransport
    /// The #459 in-flight ledger the upload sheet claims a token from — `nil`
    /// in previews that don't exercise the lifecycle.
    private let activity: TransferActivity?
    private let deviceName: String
    private let onDelete: (() -> Void)?
    private let onRename: ((String) -> Void)?
    private let onUploaded: ((DeviceObjectID?, UInt32) -> Void)?
    private let isRide: Bool

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
        onDelete: (() -> Void)? = nil,
        onRename: ((String) -> Void)? = nil,
        onUploaded: ((DeviceObjectID?, UInt32) -> Void)? = nil
    ) {
        _model = State(initialValue: RouteDetailModel(
            transport: transport, dressing: dressing,
            preloadedDetail: preloadedDetail, plannedGeometry: plannedGeometry,
            deviceObjectID: deviceObjectID, provenCommittedCRC: provenCommittedCRC,
            rideGeometry: rideGeometry
        ))
        self.transport = transport
        self.activity = activity
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
                    activity: activity,
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

/// Owns a stable model for the presented E1 cover, and turns Save into the
/// summary `MainScreenModel` lands in Planned. Upload presents the B5 sheet:
/// a completed upload also saves the route ("Uploading saves it too"), and
/// the cover closes once the sheet does.
struct ImportLandingHost: View {
    @State private var model: RouteDetailModel
    @State private var uploadRequest: UploadRequest?
    @State private var uploadCompleted = false
    private let transport: any DeviceTransport
    /// The #459 in-flight ledger the upload sheet claims a token from.
    private let activity: TransferActivity?
    private let deviceName: String
    private let noDevicePaired: Bool
    private let onSave: (RouteDetail) -> Void
    private let onUploaded: (RouteDetail, DeviceObjectID?, UInt32) -> Void
    private let onPair: (RouteDetail) -> Void
    private let onCancel: () -> Void

    init(
        transport: any DeviceTransport,
        activity: TransferActivity? = nil,
        route: ImportedRoute,
        fileName: String,
        deviceName: String,
        noDevicePaired: Bool,
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
        onSave: @escaping (RouteDetail) -> Void,
        onUploaded: @escaping (RouteDetail, DeviceObjectID?, UInt32) -> Void,
        onPair: @escaping (RouteDetail) -> Void,
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
                    activity: activity,
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
            // the F₂ sheet also closes the landing. A canceled upload stays
            // on E1, still unsaved.
            onDismiss: { if uploadCompleted { onCancel() } }
        ) { request in
            UploadSheetView(model: request.model)
        }
    }
}
