import SwiftUI
import OBCDomain
import OBCTransport

/// The detail screen for a route or ride. One view, three dressings:
/// planned, tracked, and imported.
public struct RouteDetailView: View {
    @Bindable private var model: RouteDetailModel
    private let deviceName: String
    private let onUpload: () -> Void
    private let onDelete: (() -> Void)?
    private let onRename: ((String) -> Void)?
    private let onReverse: (() -> Void)?
    private let onBikeTypeChange: ((BikeType) -> Void)?
    private let onSaveToPlanned: (() -> Void)?
    private let noDevicePaired: Bool
    private let onPair: (() -> Void)?
    /// An optional row shown above the actions in the imported dressing.
    private let importAccessory: AnyView?

    @State private var renameShown = false
    @State private var renameDraft = ""
    @State private var deleteConfirmShown = false
    @State private var waypointsExpanded = false
    @State private var mapShown = false

    @Environment(\.obcIsOnline) private var isOnline

    public init(
        model: RouteDetailModel,
        deviceName: String,
        onUpload: @escaping () -> Void = {},
        onDelete: (() -> Void)? = nil,
        onRename: ((String) -> Void)? = nil,
        onReverse: (() -> Void)? = nil,
        onBikeTypeChange: ((BikeType) -> Void)? = nil,
        onSaveToPlanned: (() -> Void)? = nil,
        noDevicePaired: Bool = false,
        onPair: (() -> Void)? = nil,
        importAccessory: AnyView? = nil
    ) {
        self.model = model
        self.deviceName = deviceName
        self.onUpload = onUpload
        self.onDelete = onDelete
        self.onRename = onRename
        self.onReverse = onReverse
        self.onBikeTypeChange = onBikeTypeChange
        self.onSaveToPlanned = onSaveToPlanned
        self.noDevicePaired = noDevicePaired
        self.onPair = onPair
        self.importAccessory = importAccessory
    }

    public var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if let line = model.importedFromLine {
                    importedBanner(line)
                }

                hero

                titleBlock

                OBCStatStrip(model.stats)

                if case .planned = model.dressing {
                    bikeTypeRow
                }

                if !model.sensorRows.isEmpty {
                    OBCGroupedSection {
                        ForEach(model.sensorRows) { row in
                            OBCListRow(
                                label: row.label,
                                value: row.value,
                                showsDivider: row.id != model.sensorRows.last?.id
                            )
                        }
                    }
                    .padding(.top, 12)
                    .accessibilityIdentifier("detail.sensorSummary")
                }

                if !model.waypoints.isEmpty {
                    OBCDisclosureRow(
                        systemImage: "mappin.and.ellipse",
                        label: waypointsLabel,
                        value: "\(model.waypoints.count)",
                        isExpanded: $waypointsExpanded,
                        headerAccessibilityID: "detail.waypoints"
                    ) {
                        WaypointsDropdownContent(waypoints: model.waypoints)
                            .accessibilityIdentifier("detail.waypointsList")
                    }
                    .padding(.top, 12)
                }

                if !model.elevationProfile.isEmpty {
                    OBCEyebrow("Elevation profile")
                        .padding(.top, 18)
                        .padding(.bottom, 4)
                    ElevationProfileView(samples: model.elevationProfile)
                }

                if case .tracked = model.dressing {
                    servicesBlock
                }
                actions
            }
            .padding(.horizontal, 20)
            .padding(.bottom, 24)
        }
        .background(OBCTheme.parchment.ignoresSafeArea())
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("detail.screen")
        #if os(iOS)
        .fullScreenCover(isPresented: $mapShown) { trackMapCover }
        #else
        .sheet(isPresented: $mapShown) { trackMapCover }
        #endif
        .obcRenameAlert(
            renameTitle,
            isPresented: $renameShown,
            name: $renameDraft,
            onSave: {
                if model.rename(to: renameDraft) { onRename?(model.name) }
            }
        )
        .task { model.start() }
    }

    /// Offline keeps the grid and no tap: a map with no network path is blank.
    private var canExpandMap: Bool {
        isOnline && !model.mapCoordinates.isEmpty
    }

    @ViewBuilder
    private var hero: some View {
        let preview = MapTrackPreviewView(
            model.preview,
            style: .hero,
            tag: model.tag.text,
            tagColor: model.tag.isAccent ? OBCTheme.forest : OBCTheme.inkSoft,
            waypoints: model.waypoints,
            totalDistanceMeters: model.distanceMeters
        )
        .frame(height: 214)

        if canExpandMap {
            Button { mapShown = true } label: {
                preview
                    // The map ignores hits, so the whole hero is the tap target.
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("detail.expandMap")
            .accessibilityLabel("Open full map")
        } else {
            preview
        }
    }

    private var trackMapCover: some View {
        TrackMapView(
            coordinates: model.mapCoordinates,
            waypoints: model.waypoints,
            title: model.name,
            onClose: { mapShown = false }
        )
    }

    private func importedBanner(_ line: String) -> some View {
        HStack(spacing: 7) {
            Image(systemName: "square.and.arrow.up")
                .font(.system(size: 12, weight: .bold))
            Text(line.uppercased())
                .font(.obcMono(size: 11, weight: .semibold))
                .kerning(1)
        }
        .foregroundStyle(OBCTheme.coral)
        .padding(.top, 14)
        .padding(.bottom, 10)
        .padding(.horizontal, 2)
        .accessibilityIdentifier("detail.importedFrom")
    }

    private var titleBlock: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(alignment: .top, spacing: 10) {
                Text(model.name)
                    .font(.obcSerif(size: 26))
                    .foregroundStyle(OBCTheme.ink)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .accessibilityIdentifier("detail.title")
                if model.isRenamable {
                    Button {
                        renameDraft = model.name
                        renameShown = true
                    } label: {
                        Image(systemName: "pencil")
                            .font(.system(size: 15, weight: .medium))
                            .foregroundStyle(OBCTheme.inkSoft)
                            .frame(width: 32, height: 32)
                            .background(OBCTheme.panel)
                            .clipShape(Circle())
                            .overlay(Circle().strokeBorder(OBCTheme.line))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(renameTitle)
                    .accessibilityIdentifier("detail.rename")
                }
            }
            if let subtitle = model.subtitle {
                Text(subtitle)
                    .font(.system(size: 14))
                    .foregroundStyle(OBCTheme.inkSoft)
            }
        }
        .padding(.top, 16)
        .padding(.bottom, 12)
    }

    private var bikeTypeRow: some View {
        OBCGroupedSection {
            Menu {
                Picker("Bike type", selection: Binding(
                    get: { model.bikeType },
                    set: { type in
                        model.setBikeType(type)
                        onBikeTypeChange?(type)
                    }
                )) {
                    ForEach(BikeType.allCases, id: \.self) { Text($0.name).tag($0) }
                }
            } label: {
                OBCListRow(label: "Bike type", value: model.bikeType.name, showsDivider: false) {
                    Image(systemName: "chevron.up.chevron.down")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(OBCTheme.inkFaint)
                }
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("detail.bikeType")
        }
        .padding(.top, 12)
    }

    /// Connected services. The affordance is inert until the services land.
    private var servicesBlock: some View {
        OBCConnectedServicesBlock(services: [
            OBCServiceStatus(
                name: "Strava", systemImage: "bolt.fill", tileColor: OBCTheme.coral,
                state: .uploaded("Uploaded on import")
            ),
            OBCServiceStatus(
                name: "Komoot", systemImage: "location.circle", tileColor: OBCTheme.wood,
                state: .notUploaded("Not uploaded")
            ),
        ])
        .padding(.top, 22)
        .accessibilityIdentifier("detail.services")
    }
    @ViewBuilder
    private var actions: some View {
        VStack(spacing: 10) {
            switch model.dressing {
            case .planned:
                uploadButton
                if let onReverse {
                    // Reverse lands a copy; the original direction stays.
                    Button("Reverse", action: onReverse)
                        .buttonStyle(.obcGhost)
                        .accessibilityIdentifier("detail.reverse")
                }
                Button("Delete route") { deleteConfirmShown = true }
                    .buttonStyle(.obcDestructive)
                    .accessibilityIdentifier("detail.delete")
                    // Anchored to the button: on the scroll root the dialog pops
                    // up mid-screen.
                    .obcDestructiveConfirm(
                        "Delete \"\(model.name)\"?",
                        isPresented: $deleteConfirmShown,
                        message: "Removes it from your library. If it's already on the device, it stays there.",
                        actionTitle: "Delete route",
                        onConfirm: { onDelete?() }
                    )
            case .imported where noDevicePaired:
                // A share can arrive before pairing: the route saves now and
                // uploads later. Trips are app-local, so Add-to-trip still works.
                importAccessory
                OBCInlineBanner(
                    systemImage: "antenna.radiowaves.left.and.right.slash",
                    title: "No device paired yet.",
                    message: "Save it now — upload once you pair."
                )
                .padding(.bottom, 4)
                Button("Save to Planned") { onSaveToPlanned?() }
                    .buttonStyle(.obcPrimary)
                    .accessibilityIdentifier("detail.saveToPlanned")
                Button("Pair a device") { onPair?() }
                    .buttonStyle(.obcGhost)
                    .accessibilityIdentifier("detail.pairDevice")
            case .imported:
                importAccessory
                uploadButton
                Button("Save to Planned") { onSaveToPlanned?() }
                    .buttonStyle(.obcGhost)
                    .accessibilityIdentifier("detail.saveToPlanned")
                Text("Uploading saves it too. Tap Cancel to discard.")
                    .font(.system(size: 12))
                    .foregroundStyle(OBCTheme.inkFaint)
                    .frame(maxWidth: .infinity)
                    .padding(.top, 2)
            case .tracked:
                // The services block above carries the per-ride upload.
                Button("Delete ride") { deleteConfirmShown = true }
                    .buttonStyle(.obcDestructive)
                    .accessibilityIdentifier("detail.delete")
                    .obcDestructiveConfirm(
                        "Delete \"\(model.name)\"?",
                        isPresented: $deleteConfirmShown,
                        message: "Moves it to Recently Deleted. The ride stays on the device.",
                        actionTitle: "Delete ride",
                        onConfirm: { onDelete?() }
                    )
            }
        }
        .padding(.top, 20)
    }

    private var uploadButton: some View {
        let state = model.deviceCopyState
        return Button {
            onUpload()
        } label: {
            switch state {
            case .notOnDevice:
                Label("Upload to \(deviceName)", systemImage: "square.and.arrow.up")
            case .outdated:
                Label("Update on \(deviceName)", systemImage: "arrow.triangle.2.circlepath")
            case .upToDate:
                Label("Up to date on \(deviceName)", systemImage: "checkmark.circle")
            }
        }
        .buttonStyle(.obcPrimary)
        .disabled(!model.canUpload || state == .upToDate)
        .accessibilityIdentifier("detail.upload")
    }

    private var waypointsLabel: String {
        if case .imported = model.dressing { return "Waypoints from file" }
        return "Waypoints"
    }

    private var renameTitle: String {
        if case .tracked = model.dressing { return "Rename ride" }
        return "Rename route"
    }
}

/// The import landing: the detail body framed by Cancel and Save chrome.
/// Shown full-screen when a route file decodes.
public struct ImportLandingView: View {
    private let model: RouteDetailModel
    private let deviceName: String
    private let onUpload: () -> Void
    private let onSave: () -> Void
    private let onCancel: () -> Void
    private let noDevicePaired: Bool
    private let onPair: () -> Void
    private let importAccessory: AnyView?

    public init(
        model: RouteDetailModel,
        deviceName: String,
        onUpload: @escaping () -> Void = {},
        onSave: @escaping () -> Void = {},
        onCancel: @escaping () -> Void = {},
        noDevicePaired: Bool = false,
        onPair: @escaping () -> Void = {},
        importAccessory: AnyView? = nil
    ) {
        self.model = model
        self.deviceName = deviceName
        self.onUpload = onUpload
        self.onSave = onSave
        self.onCancel = onCancel
        self.noDevicePaired = noDevicePaired
        self.onPair = onPair
        self.importAccessory = importAccessory
    }

    public var body: some View {
        NavigationStack {
            RouteDetailView(
                model: model,
                deviceName: deviceName,
                onUpload: onUpload,
                onSaveToPlanned: onSave,
                noDevicePaired: noDevicePaired,
                onPair: onPair,
                importAccessory: importAccessory
            )
            .navigationTitle("Imported route")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", action: onCancel)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save", action: onSave)
                        .fontWeight(.semibold)
                }
            }
        }
        .tint(OBCTheme.tint)
    }
}

#if DEBUG
#Preview("E2 · planned") {
    // OBCUI cannot import OBCMock, so previews use placeholder data.
    NavigationStack {
        RouteDetailView(
            model: RouteDetailModel(
                transport: PreviewNoopTransport(),
                dressing: .planned(RouteSummary(
                    id: RouteID("preview"), name: "Blue Mounds Backroads",
                    distanceMeters: 84_700, elevationGainMeters: 1_240,
                    estimatedDuration: 16_800, pointCount: 2_183,
                    trackPreview: .obcSample
                ))
            ),
            deviceName: "Trailhead"
        )
        .navigationTitle("Route")
    }
}

#Preview("E3 · tracked") {
    NavigationStack {
        RouteDetailView(
            model: RouteDetailModel(
                transport: PreviewNoopTransport(),
                dressing: .tracked(RideSummary(
                    id: RideID("preview"), name: "Kettle Moraine Loop",
                    date: Date().addingTimeInterval(-86_400 * 1 - 3_600 * 3),
                    distanceMeters: 58_200, movingTime: 10_260,
                    averageSpeedMps: 5.67, climbMeters: 812,
                    trackPreview: .obcSample
                ))
            ),
            deviceName: "Trailhead"
        )
        .navigationTitle("Ride")
    }
}

private struct PreviewNoopTransport: DeviceLink, DeviceObjects {
    var state: AsyncStream<ConnectionState> { AsyncStream { $0.finish() } }
    func connect() async throws {}
    func disconnect() async {}
    func deviceInfo() async throws -> DeviceInfo { DeviceInfo(name: "Preview", firmwareVersion: "0") }
    func listRoutes() async throws -> [RouteCatalogEntry] { [] }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { throw DeviceError.readFailed }
    func uploadRoute(_ route: RouteBlob) -> TransferHandle { .immediatelyFinished(.failed(.notConnected)) }
    func deleteRoute(_ id: DeviceObjectID) async throws {}
    func listRides() async throws -> RideCatalog { RideCatalog(rides: []) }
    func rideDetail(_ id: RideID) async throws -> RideDetail { throw DeviceError.readFailed }
    func downloadRides(_ ids: [RideID]) -> RideDownload { .finished() }
}
#endif
