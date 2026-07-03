import SwiftUI
import OBCDomain
import OBCTransport

/// The route-detail screen (B4) in the finalized **profile layout**: track hero
/// → title (pencil = H12 on planned/tracked) → inline stat strip → Waypoints
/// disclosure (→ W1) → elevation profile → actions **inline in the scroll**
/// (design rule: no floating/sticky button). One view, three dressings — E2
/// planned, E3 tracked, E1 import landing (framed by `ImportLandingView`).
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
    private let onSaveToPlanned: (() -> Void)?
    private let noDevicePaired: Bool
    private let onPair: (() -> Void)?

    @State private var renameShown = false
    @State private var renameDraft = ""
    @State private var deleteConfirmShown = false
    @State private var waypointsShown = false

    public init(
        model: RouteDetailModel,
        deviceName: String,
        onUpload: @escaping () -> Void = {},
        onDelete: (() -> Void)? = nil,
        onRename: ((String) -> Void)? = nil,
        onSaveToPlanned: (() -> Void)? = nil,
        noDevicePaired: Bool = false,
        onPair: (() -> Void)? = nil
    ) {
        self.model = model
        self.deviceName = deviceName
        self.onUpload = onUpload
        self.onDelete = onDelete
        self.onRename = onRename
        self.onSaveToPlanned = onSaveToPlanned
        self.noDevicePaired = noDevicePaired
        self.onPair = onPair
    }

    public var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if let line = model.importedFromLine {
                    importedBanner(line)
                }

                TrackPreviewView(
                    model.preview,
                    style: .hero,
                    tag: model.tag.text,
                    tagColor: model.tag.isAccent ? OBCTheme.forest : OBCTheme.inkSoft
                )
                .frame(height: 214)

                titleBlock

                OBCStatStrip(model.stats)

                if !model.waypoints.isEmpty {
                    OBCDisclosureRow(
                        systemImage: "mappin.and.ellipse",
                        label: waypointsLabel,
                        value: "\(model.waypoints.count)"
                    ) {
                        waypointsShown = true
                    }
                    .accessibilityIdentifier("detail.waypoints")
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
        .navigationDestination(isPresented: $waypointsShown) {
            WaypointsScreen(
                waypoints: model.waypoints,
                preview: model.preview,
                totalDistanceMeters: model.distanceMeters
            )
        }
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

    @ViewBuilder
    private var actions: some View {
        VStack(spacing: 10) {
            switch model.dressing {
            case .planned:
                uploadButton
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
                // saves; upload waits until a device exists.
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
                        message: "Removes it from your phone. The ride stays on the device.",
                        actionTitle: "Delete ride",
                        onConfirm: { onDelete?() }
                    )
            }
        }
        .padding(.top, 20)
    }

    private var uploadButton: some View {
        Button {
            onUpload()
        } label: {
            Label("Upload to \(deviceName)", systemImage: "square.and.arrow.up")
        }
        .buttonStyle(.obcPrimary)
        // Link-bound, so it dims with the link (S4) — the top bar / banner
        // already tell the disconnect story.
        .disabled(!model.canUpload)
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

/// W1 — the waypoints list pushed from the disclosure row: the mini track with
/// numbered pins, the rows in ride order, and the provenance footer.
struct WaypointsScreen: View {
    let waypoints: [Waypoint]
    let preview: TrackPreview?
    let totalDistanceMeters: Double

    var body: some View {
        ScrollView {
            VStack(spacing: 0) {
                WaypointsListView(
                    waypoints: waypoints,
                    preview: preview,
                    totalDistanceMeters: totalDistanceMeters
                )
                Text("Waypoints come from the route file and are uploaded to the device with it.")
                    .font(.system(size: 12))
                    .foregroundStyle(OBCTheme.inkFaint)
                    .multilineTextAlignment(.center)
                    .padding(.top, 16)
                    .padding(.horizontal, 24)
            }
            .padding(.horizontal, 20)
            .padding(.top, 14)
            .padding(.bottom, 24)
        }
        .background(OBCTheme.parchment.ignoresSafeArea())
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("waypoints.screen")
        .navigationTitle("Waypoints")
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
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

    public init(
        model: RouteDetailModel,
        deviceName: String,
        onUpload: @escaping () -> Void = {},
        onSave: @escaping () -> Void = {},
        onCancel: @escaping () -> Void = {},
        noDevicePaired: Bool = false,
        onPair: @escaping () -> Void = {}
    ) {
        self.model = model
        self.deviceName = deviceName
        self.onUpload = onUpload
        self.onSave = onSave
        self.onCancel = onCancel
        self.noDevicePaired = noDevicePaired
        self.onPair = onPair
    }

    public var body: some View {
        NavigationStack {
            RouteDetailView(
                model: model,
                deviceName: deviceName,
                onUpload: onUpload,
                onSaveToPlanned: onSave,
                noDevicePaired: noDevicePaired,
                onPair: onPair
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
    func connect() async throws {}
    func disconnect() async {}
    func deviceInfo() async throws -> DeviceInfo { DeviceInfo(name: "Preview", firmwareVersion: "0") }
    func readConfig() async throws -> DeviceConfig { DeviceConfig(name: "Preview") }
    func writeConfig(_ config: DeviceConfig) async throws {}
    func listRoutes() async throws -> [RouteSummary] { [] }
    func routeDetail(_ id: RouteID) async throws -> RouteDetail { throw DeviceError.readFailed }
    func uploadRoute(_ route: RouteBlob) -> TransferHandle { .immediatelyFinished(.failed(.notConnected)) }
    func deleteRoute(_ id: RouteID) async throws {}
    func listRides() async throws -> [RideSummary] { [] }
    func rideDetail(_ id: RideID) async throws -> RideDetail { throw DeviceError.readFailed }
    func downloadRides(_ ids: [RideID]) -> RideDownload { .finished() }
    func readDiagnostics() async throws -> Data { Data() }
}
#endif
