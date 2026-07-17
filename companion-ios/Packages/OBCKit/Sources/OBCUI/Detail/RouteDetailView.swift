import SwiftUI
import OBCDomain
import OBCTransport

/// The route-detail screen (B4) in the finalized **profile layout**: track hero
/// (waypoints pinned as numbered markers) → title (pencil = H12) → inline stat
/// strip → Waypoints dropdown (W1, folds out in place) → elevation profile →
/// actions **inline in the scroll** (design rule: no floating/sticky button).
/// One view, three dressings — E2 planned, E3 tracked, E1 import landing
/// (framed by `ImportLandingView`).
///
/// What the actions *open* stays seams the composition root wires: upload →
/// the B5 sheet, delete → pop after `MainScreenModel.deleteRoute`, save →
/// `addImportedRoute`.
public struct RouteDetailView: View {
    @Bindable private var model: RouteDetailModel
    private let deviceName: String
    private let onUpload: () -> Void
    private let onDelete: (() -> Void)?
    private let onRename: ((String) -> Void)?
    private let onReverse: (() -> Void)?
    private let onSaveToPlanned: (() -> Void)?
    private let noDevicePaired: Bool
    private let onPair: (() -> Void)?
    /// An optional row shown above the imported-dressing actions (E1) — the TR7
    /// "Add to trip" row the import landing injects; `nil` on E2/E3 and when no
    /// trip filing is offered.
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

                if model.showsRetentionRow {
                    retentionSection
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

    // MARK: Pieces

    /// Whether the hero can open the full interactive map: real geometry **and**
    /// a network path (offline keeps the grid, no tap — never a blank map). #294.
    private var canExpandMap: Bool {
        isOnline && !model.mapCoordinates.isEmpty
    }

    /// The track hero — a basemap when online, the grid otherwise, with the
    /// route's waypoints pinned as numbered markers either way. When a map is
    /// available, tapping anywhere on it opens the full-screen `TrackMapView`
    /// (no separate expand affordance — the whole hero is the tap target).
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
                    // The map ignores hits (the tap is ours), so make the whole
                    // hero the button's tap target.
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

    /// The E1 provenance line above the hero — mono uppercase in coral.
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

    /// E3's connected-services sync block — shipped coming-soon; the per-ride
    /// Upload affordance is the designed seam (no-op until services land).
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

    /// The Auto-delete control (epic #638 S7) — shown for a planned route on the
    /// device: the desired level with the device's expiry truth beneath, editable
    /// in place (the main model pushes live or at the next reconcile).
    private var retentionSection: some View {
        OBCGroupedSection {
            OBCRetentionRow(
                selection: model.retentionValue,
                detailLine: model.expiryLine,
                showsDivider: false,
                accessibilityID: "detail.autoDelete",
                onSelect: { model.editRetention($0) }
            )
        }
        .padding(.top, 12)
    }

    @ViewBuilder
    private var actions: some View {
        VStack(spacing: 10) {
            switch model.dressing {
            case .planned:
                uploadButton
                if let onReverse {
                    // #503 — a whole-route flip lands a reversed copy alongside
                    // the original; the rider keeps both directions.
                    Button("Reverse", action: onReverse)
                        .buttonStyle(.obcGhost)
                        .accessibilityIdentifier("detail.reverse")
                }
                Button("Delete route") { deleteConfirmShown = true }
                    .buttonStyle(.obcDestructive)
                    .accessibilityIdentifier("detail.delete")
                    // Anchored to the button itself — hung off the scroll root
                    // the H1 dialog pops up mid-screen over the title.
                    .obcDestructiveConfirm(
                        "Delete \"\(model.name)\"?",
                        isPresented: $deleteConfirmShown,
                        message: "Removes it from your library. If it's already on the device, it stays there.",
                        actionTitle: "Delete route",
                        onConfirm: { onDelete?() }
                    )
            case .imported where noDevicePaired:
                // H4 — a share can arrive before pairing: the route still
                // saves; upload waits until a device exists. A trip is app-local,
                // so the Add-to-trip row works with no device just the same.
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
                // The services block above carries the per-ride upload; delete
                // matches the planned dressing (one-tap, so it confirms via H1).
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

    /// Upload ↔ Update ↔ up-to-date, off the proven device-copy state: a fresh
    /// route uploads, a changed one (rename, re-import) **updates the copy in
    /// place**, and a byte-identical one has nothing to push — the button says
    /// so and stays disabled. Link-bound either way (S4: dims with the link).
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

    // No message under the rename title — what a rename does is obvious. The
    // name still propagates everywhere (device on next upload, syncs, services).
    private var renameTitle: String {
        if case .tracked = model.dressing { return "Rename ride" }
        return "Rename route"
    }
}

/// E1 — the import landing: the same detail body framed by **Cancel / Save**
/// chrome. Presented full-screen by the composition root when a route file
/// decodes (Files pick, share sheet, or the `-OBCImportSample` hook). With no
/// device paired it wears the H4 framing instead — the no-device banner plus
/// **Save to Planned** / **Pair a device** in place of Upload.
public struct ImportLandingView: View {
    private let model: RouteDetailModel
    private let deviceName: String
    private let onUpload: () -> Void
    private let onSave: () -> Void
    private let onCancel: () -> Void
    private let noDevicePaired: Bool
    private let onPair: () -> Void
    /// The TR7 "Add to trip" row, injected above the E1 actions (`nil` = no
    /// trip filing offered).
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
    // Preview-only placeholder data (OBCUI can't import OBCMock — see the app
    // target's RootView previews for the transport-driven screens).
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

/// Inert transport for `#Preview` construction only.
private struct PreviewNoopTransport: DeviceTransport {
    var state: AsyncStream<ConnectionState> { AsyncStream { $0.finish() } }
    var battery: AsyncStream<Int> { AsyncStream { $0.finish() } }
    var storeChanges: AsyncStream<StoreChanged> { AsyncStream { $0.finish() } }
    func connect() async throws {}
    func disconnect() async {}
    func deviceInfo() async throws -> DeviceInfo { DeviceInfo(name: "Preview", firmwareVersion: "0") }
    func readConfig() async throws -> DeviceConfig { DeviceConfig(name: "Preview") }
    func writeConfig(_ config: DeviceConfig) async throws {}
    func listRoutes() async throws -> [RouteCatalogEntry] { [] }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { throw DeviceError.readFailed }
    func uploadRoute(_ route: RouteBlob) -> TransferHandle { .immediatelyFinished(.failed(.notConnected)) }
    func deleteRoute(_ id: DeviceObjectID) async throws {}
    func listRides() async throws -> RideCatalog { RideCatalog(rides: []) }
    func rideDetail(_ id: RideID) async throws -> RideDetail { throw DeviceError.readFailed }
    func downloadRides(_ ids: [RideID]) -> RideDownload { .finished() }
    func readDiagnostics() async throws -> Data { Data() }
}
#endif
