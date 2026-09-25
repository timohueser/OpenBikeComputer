import SwiftUI
import OBCDomain
import OBCTransport

/// The detail screen for a route or ride. One view, four dressings: planned, trip day, tracked,
/// and imported. A route page leads with what the device holds and the one action, then its
/// elevation profile and the ledger in the device route overview's layout. A ride page leads with
/// its three totals and timeline, with secondary statistics in a disclosure.
public struct RouteDetailView: View {
    @Bindable private var model: RouteDetailModel
    private let deviceName: String
    private let onUpload: () -> Void
    private let onRename: ((String) -> Void)?
    /// Set by a host that rebuilds this view while it is on screen: the host presents the rename
    /// sheet above the rebuild, because a rebuild closes a sheet presented from inside it.
    private let onRenameTap: (() -> Void)?
    private let onBikeTypeChange: ((BikeType) -> Void)?
    private let noDevicePaired: Bool
    private let onPair: (() -> Void)?
    /// The imported dressing's save choices, directly under the title.
    private let importAccessory: AnyView?
    /// A tracked ride's photos.
    private let photos: RidePhotosModel?
    /// A tracked ride's day note.
    private let dayNote: DayNoteModel?
    /// The quiet rows over a ride's ledger, such as a merge suggestion.
    private let quietRows: AnyView?

    @State private var renameShown = false
    @State private var statisticsExpanded = false
    @State private var waypointsExpanded = false
    @State private var mapShown = false
    /// The timeline's cursor, in metres along the ride; nil before the first scrub.
    @State private var cursor: Double?
    @State private var openChannel: RideTimeline.Channel?
    @State private var replayShown = false
    @State private var replayPreparing = false
    @State private var replayContent: ReplayContent?

    @Environment(\.obcIsOnline) private var isOnline

    public init(
        model: RouteDetailModel,
        deviceName: String,
        onUpload: @escaping () -> Void = {},
        onRename: ((String) -> Void)? = nil,
        onRenameTap: (() -> Void)? = nil,
        onBikeTypeChange: ((BikeType) -> Void)? = nil,
        noDevicePaired: Bool = false,
        onPair: (() -> Void)? = nil,
        importAccessory: AnyView? = nil,
        photos: RidePhotosModel? = nil,
        dayNote: DayNoteModel? = nil,
        quietRows: AnyView? = nil
    ) {
        self.model = model
        self.deviceName = deviceName
        self.onUpload = onUpload
        self.onRename = onRename
        self.onRenameTap = onRenameTap
        self.onBikeTypeChange = onBikeTypeChange
        self.noDevicePaired = noDevicePaired
        self.onPair = onPair
        self.importAccessory = importAccessory
        self.photos = photos
        self.dayNote = dayNote
        self.quietRows = quietRows
    }

    public var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                hero

                titleBlock

                if !model.summaryStats.isEmpty {
                    OBCStatSummary(stats: model.summaryStats)
                        .padding(.top, 16)
                }

                switch model.dressing {
                case .planned:
                    DeviceCopyStatus(
                        state: model.deviceCopyState,
                        connection: model.connection,
                        deviceName: deviceName,
                        onSend: onUpload
                    )
                    .padding(.top, 14)
                case .imported:
                    importAccessory
                        .padding(.top, 14)
                case .tripDay, .tracked:
                    EmptyView()
                }

                quietRows

                if let timeline = model.timeline {
                    RideTimelineCard(timeline: timeline, cursor: $cursor, photoTicks: photos?.tickFractions ?? []) {
                        openChannel = $0
                    }
                    .padding(.top, 20)
                } else if !model.elevationProfile.isEmpty {
                    OBCEyebrow("Elevation")
                        .padding(.top, 20)
                        .padding(.bottom, 6)
                    ElevationProfileView(samples: model.elevationProfile)
                }

                if model.canReplay {
                    replayButton
                        .padding(.top, 16)
                }

                if case .tracked = model.dressing {
                    OBCDisclosureRow(
                        systemImage: "chart.bar",
                        label: "More statistics",
                        isExpanded: $statisticsExpanded,
                        headerAccessibilityID: "detail.statistics"
                    ) {
                        VStack(alignment: .leading, spacing: 12) {
                            OBCLedger(model.stats)
                            if let timeline = model.timeline {
                                RideZonesCard(timeline: timeline)
                            }
                        }
                        .padding(.horizontal, -16)
                        .padding(.top, 8)
                    }
                    .padding(.top, 20)
                } else if !model.stats.isEmpty {
                    OBCLedger(model.stats)
                        .padding(.top, 20)
                }

                if let photos {
                    RidePhotoStripSection(model: photos, preview: model.preview)
                }
                if let dayNote {
                    DayNoteEntry(model: dayNote, photos: photos)
                }
                RideOffers(photos: photos, dayNote: dayNote)

                if case .planned = model.dressing {
                    bikeTypeRow
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

                if case .tracked = model.dressing {
                    bikeTypeRow
                }
                actions
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 24)
        }
        .background(OBCTheme.page.ignoresSafeArea())
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("detail.screen")
        #if os(iOS)
        .fullScreenCover(isPresented: $mapShown) { trackMapCover }
        .fullScreenCover(isPresented: $replayShown) {
            if let replayContent { ReplayPlayerView(content: replayContent) }
        }
        #else
        .sheet(isPresented: $mapShown) { trackMapCover }
        .sheet(isPresented: $replayShown) {
            if let replayContent { ReplayPlayerView(content: replayContent) }
        }
        #endif
        .sheet(item: $openChannel) { channel in
            if let timeline = model.timeline {
                RideChannelSheet(timeline: timeline, channel: channel, cursor: $cursor)
            }
        }
        .obcRenameSheet(
            renameTitle,
            isPresented: $renameShown,
            name: model.name,
            onSave: {
                if model.rename(to: $0) { onRename?(model.name) }
            }
        )
        .task { model.start() }
        .task { await photos?.start() }
        .task { await dayNote?.start() }
    }

    /// Offline keeps the sketch and no tap: a map with no network path is blank.
    private var canExpandMap: Bool {
        isOnline && !model.mapCoordinates.isEmpty
    }

    @ViewBuilder
    private var hero: some View {
        let preview = MapTrackPreviewView(
            model.preview,
            ink: model.ink,
            style: .hero,
            waypoints: model.waypoints,
            totalDistanceMeters: model.distanceMeters,
            photoPins: photos?.pinCoordinates ?? [],
            cursor: cursor.flatMap { model.timeline?.line.coordinate(at: $0) }
        )
        .frame(height: 200)
        .overlay(alignment: .bottomTrailing) {
            Label(
                canExpandMap ? "Open map" : (isOnline ? "Map unavailable" : "Offline preview"),
                systemImage: canExpandMap ? "arrow.up.left.and.arrow.down.right" : (isOnline ? "map" : "wifi.slash")
            )
            .font(.system(.subheadline, weight: .medium))
            .foregroundStyle(OBCTheme.ink)
            .padding(10)
            .background(OBCTheme.surface, in: Capsule())
            .padding(10)
        }
        .padding(.top, 8)

        if canExpandMap {
            Button { mapShown = true } label: {
                preview
                    // The map ignores hits, so the whole hero is the tap target.
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("detail.expandMap")
            .accessibilityLabel("Open map")
            .accessibilityHint("Opens the full map")
        } else {
            preview
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("\(mapLabel). \(isOnline ? "Map unavailable" : "Offline preview")")
        }
    }

    private var mapLabel: String {
        if case .tracked = model.dressing { return "Map of the ride" }
        return "Map of the route"
    }

    private var trackMapCover: some View {
        TrackMapView(
            coordinates: model.mapCoordinates,
            ink: model.ink,
            waypoints: model.waypoints,
            title: model.name,
            onClose: { mapShown = false }
        )
    }

    private var titleBlock: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(model.name)
                    .font(.system(.title, weight: .bold))
                    .foregroundStyle(OBCTheme.ink)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .accessibilityAddTraits(.isHeader)
                    .accessibilityIdentifier("detail.title")
                if model.isRenamable {
                    Button {
                        if let onRenameTap { onRenameTap() } else { renameShown = true }
                    } label: {
                        Image(systemName: "pencil")
                            .font(.system(.body, weight: .medium))
                            .foregroundStyle(OBCTheme.secondary)
                            .obcFixedGeometryType()
                            .frame(width: 44, height: 44)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    // The glyph's optical edge lines up with the page margin.
                    .padding(.trailing, -12)
                    .accessibilityLabel(renameTitle)
                    .accessibilityIdentifier("detail.rename")
                }
            }
            if let subtitle = model.subtitle {
                Text(subtitle)
                    .font(.system(.subheadline))
                    .foregroundStyle(OBCTheme.secondary)
                    .accessibilityIdentifier("detail.source")
            }
        }
        .padding(.top, 14)
    }

    private var replayButton: some View {
        Button {
            replayPreparing = true
            Task {
                replayContent = await model.replayContent(
                    photos: photos?.photos ?? [], thumbnails: photos?.thumbnails ?? [:])
                replayPreparing = false
                replayShown = replayContent != nil
            }
        } label: {
            if replayPreparing {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Preparing replay…")
                }
            } else {
                Label("Replay ride", systemImage: "play.circle")
            }
        }
        .buttonStyle(.obcGhost)
        .disabled(replayPreparing)
        .accessibilityIdentifier("detail.replay")
    }

    private var bikeTypeRow: some View {
        OBCGroupedSection {
            OBCBikeTypeRow(type: model.bikeType) { type in
                model.setBikeType(type)
                onBikeTypeChange?(type)
            }
            .accessibilityIdentifier("detail.bikeType")
        }
        .padding(.top, 12)
    }

    @ViewBuilder
    private var actions: some View {
        if case .imported = model.dressing, noDevicePaired {
            VStack(spacing: 10) {
                OBCInlineBanner(
                    systemImage: "antenna.radiowaves.left.and.right.slash",
                    title: "No device paired yet.",
                    message: "Save it now and send it after you pair."
                )
                .padding(.bottom, 4)
                Button("Pair a device") { onPair?() }
                    .buttonStyle(.obcGhost)
                    .accessibilityIdentifier("detail.pairDevice")
            }
            .padding(.top, 20)
        }
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

/// The import landing: the detail body under Cancel and "Imported route", with the choice rows.
/// Shown full-screen when a route file decodes.
public struct ImportLandingView: View {
    private let model: RouteDetailModel
    private let deviceName: String
    private let onCancel: () -> Void
    private let noDevicePaired: Bool
    private let onPair: () -> Void
    private let importAccessory: AnyView?

    public init(
        model: RouteDetailModel,
        deviceName: String,
        onCancel: @escaping () -> Void = {},
        noDevicePaired: Bool = false,
        onPair: @escaping () -> Void = {},
        importAccessory: AnyView? = nil
    ) {
        self.model = model
        self.deviceName = deviceName
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
                noDevicePaired: noDevicePaired,
                onPair: onPair,
                importAccessory: importAccessory
            )
            .navigationTitle(model.landingTitle)
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", action: onCancel)
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
    func downloadRides(_ ids: [RideID]) -> RideDownload { .finished() }
}
#endif
