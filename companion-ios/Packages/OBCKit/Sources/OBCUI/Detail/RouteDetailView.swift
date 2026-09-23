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
    /// Set by a host that rebuilds this view while it is on screen: the host presents the rename
    /// sheet above the rebuild, because a rebuild closes a sheet presented from inside it.
    private let onRenameTap: (() -> Void)?
    private let onReverse: (() -> Void)?
    private let onBikeTypeChange: ((BikeType) -> Void)?
    private let noDevicePaired: Bool
    private let onPair: (() -> Void)?
    /// The imported dressing's choice rows, under the stats.
    private let importAccessory: AnyView?
    /// A tracked ride's photos.
    private let photos: RidePhotosModel?
    /// A tracked ride's day note.
    private let dayNote: DayNoteModel?
    /// The quiet rows under a ride's stats line, such as a merge suggestion.
    private let quietRows: AnyView?

    @State private var renameShown = false
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
        onRenameTap: (() -> Void)? = nil,
        onReverse: (() -> Void)? = nil,
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
        self.onDelete = onDelete
        self.onRename = onRename
        self.onRenameTap = onRenameTap
        self.onReverse = onReverse
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
                if let line = model.importedFromLine {
                    importedBanner(line)
                }

                hero

                titleBlock

                if let photos {
                    RidePhotoOfferRow(model: photos)
                }
                if let dayNote {
                    DayNoteOfferRow(model: dayNote, photos: photos)
                }
                quietRows

                if !model.stats.isEmpty {
                    OBCStatStrip(model.stats)
                }

                if case .planned = model.dressing {
                    bikeTypeRow
                }

                if case .imported = model.dressing, let importAccessory {
                    importAccessory
                        .padding(.top, 14)
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
                    ElevationProfileView(samples: model.elevationProfile, ticks: photos?.tickFractions ?? [])
                }

                if !model.highlights.isEmpty {
                    highlightsLine
                }

                if let photos {
                    RidePhotoStripSection(model: photos, preview: model.preview)
                }
                if let dayNote {
                    DayNoteEntry(model: dayNote, photos: photos)
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
                    .padding(.top, 16)
                    .accessibilityIdentifier("detail.sensorSummary")
                }

                if case .tracked = model.dressing {
                    // Ride detail B: the ride's facts first, then sensors, bike type and services.
                    bikeTypeRow
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
            totalDistanceMeters: model.distanceMeters,
            photoPins: photos?.pinCoordinates ?? []
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
                        if let onRenameTap { onRenameTap() } else { renameShown = true }
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
            if let statsLine = model.statsLine {
                Text(statsLine)
                    .font(.obcMono(size: 15, weight: .medium))
                    .foregroundStyle(OBCTheme.ink)
                    .padding(.top, 6)
                    .accessibilityIdentifier("detail.statsLine")
            }
        }
        .padding(.top, 16)
        .padding(.bottom, 12)
    }

    private var highlightsLine: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            OBCEyebrow("Highlights")
            Text(model.highlights.joined(separator: " · "))
                .font(.obcMono(size: 13, weight: .medium))
                .foregroundStyle(OBCTheme.inkSoft)
        }
        .padding(.top, 14)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("detail.highlights")
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
                // A share can arrive before pairing: the route saves now and uploads later.
                OBCInlineBanner(
                    systemImage: "antenna.radiowaves.left.and.right.slash",
                    title: "No device paired yet.",
                    message: "Save it now — upload once you pair."
                )
                .padding(.bottom, 4)
                Button("Pair a device") { onPair?() }
                    .buttonStyle(.obcGhost)
                    .accessibilityIdentifier("detail.pairDevice")
            case .imported:
                // The rows under the stats land the route; upload is on the route or trip page.
                EmptyView()
            case .tripDay:
                // The trip page uploads and deletes the whole trip.
                EmptyView()
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
