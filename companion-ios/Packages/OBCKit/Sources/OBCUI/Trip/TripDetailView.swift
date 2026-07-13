import SwiftUI
import OBCDomain
import OBCTransport

/// The **trip page** (TR6) — behind a trip card in the routes list. Header:
/// the trip name + summed stats and the primary **Upload trip** action (wired in
/// TR8; disabled here). Below: the member routes as the **same** route cards as
/// everywhere, in stage order, each tinted with its palette color; a tap opens
/// the ordinary `RouteDetailView`. The overflow menu carries Rename, Reorder
/// stages (drag), Remove from trip (per stage), and Delete trip… (Ungroup vs
/// Delete trip & routes).
///
/// Driven straight off `MainScreenModel` (the `RecentlyDeletedView` idiom): the
/// model owns the trip edits and the library; this view binds them. It pops
/// itself (`onClose`) the moment the trip dissolves or is deleted.
public struct TripDetailView: View {
    @Bindable private var model: MainScreenModel
    private let tripID: TripID
    private let onSelectRoute: (RouteSummary) -> Void
    private let onClose: () -> Void

    /// Reorder mode (drag handles) — a plain flag mapped to `\.editMode` on iOS
    /// (that environment key is unavailable on the macOS host the package's
    /// `swift test` also builds for).
    @State private var isReordering = false
    @State private var renameShown = false
    @State private var renameDraft = ""
    @State private var deleteDialogShown = false
    /// The whole-trip upload sheet's driver (TR8), created once at the Upload tap
    /// (a model built inline in the `.sheet` closure would rebuild every body
    /// pass, restarting the queue).
    @State private var tripUploadModel: TripUploadModel?
    /// Upload tapped, catalog re-read in flight (`prepareTripUpload`) — debounces
    /// the button until the sheet's driver exists.
    @State private var isPreparingUpload = false
    /// A pending "remove the last stage" — dissolving the trip needs an inline
    /// confirm (the trip is created with ≥ 1 route; emptying it removes it).
    @State private var dissolveConfirmStage: RouteID?
    /// The full-screen interactive trip map (the route detail's hero idiom).
    @State private var mapShown = false

    @Environment(\.obcIsOnline) private var isOnline

    public init(
        model: MainScreenModel,
        tripID: TripID,
        onSelectRoute: @escaping (RouteSummary) -> Void = { _ in },
        onClose: @escaping () -> Void = {}
    ) {
        self.model = model
        self.tripID = tripID
        self.onSelectRoute = onSelectRoute
        self.onClose = onClose
    }

    private var trip: TripRecord? { model.trip(tripID) }
    private var stages: [RouteSummary] { model.tripStages(tripID) }

    public var body: some View {
        List {
            header
                .listRowSeparator(.hidden)
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets(top: 4, leading: 20, bottom: 8, trailing: 20))

            ForEach(Array(stages.enumerated()), id: \.element.id) { index, stage in
                Button { onSelectRoute(stage) } label: {
                    RouteCard(
                        route: stage,
                        onDevice: model.onDeviceState(stage.id),
                        stageAccent: OBCTheme.stageColor(index: index)
                    )
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("trip.stage.\(stage.id.rawValue)")
                .listRowSeparator(.hidden)
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 12, trailing: 20))
                .swipeActions(edge: .trailing) {
                    Button(role: .destructive) { attemptRemove(stage.id) } label: {
                        Label("Remove", systemImage: "minus.circle")
                    }
                }
                // Clip BOTH long-press lift previews — the context menu's and the
                // reorder drag's — to the card's own rounded shape, exactly as the
                // main screen's route rows do; without this the system snapshots
                // the whole rectangular row and the card floats on a stark white
                // slab. (iOS-only kind; macOS is the test host.)
                #if os(iOS)
                .contentShape(
                    [.contextMenuPreview, .dragPreview],
                    RoundedRectangle(cornerRadius: OBCTheme.radiusCard)
                )
                #endif
                // The same long-press affordance as the main screen's cards: a
                // rounded lift with a small menu (the reorder drag still starts
                // from the lift by moving, and the overflow's explicit reorder
                // mode is untouched).
                .contextMenu {
                    Button {
                        onSelectRoute(stage)
                    } label: {
                        Label("Open route", systemImage: "map")
                    }
                    Button(role: .destructive) {
                        attemptRemove(stage.id)
                    } label: {
                        Label("Remove from trip", systemImage: "minus.circle")
                    }
                }
            }
            .onMove(perform: moveStages)
            .onDelete(perform: deleteStages)
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(OBCTheme.parchment.ignoresSafeArea())
        #if os(iOS)
        .environment(\.editMode, .constant(isReordering ? .active : .inactive))
        #endif
        .navigationTitle(trip?.name ?? "Trip")
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .toolbar { overflowMenu }
        .accessibilityIdentifier("trip.screen")
        .obcRenameAlert(
            "Rename trip",
            isPresented: $renameShown,
            name: $renameDraft,
            onSave: {
                let name = renameDraft.trimmingCharacters(in: .whitespacesAndNewlines)
                if !name.isEmpty { model.renameTrip(tripID, to: name) }
            }
        )
        .confirmationDialog(
            "Delete \(trip?.name.quoted ?? "trip")?",
            isPresented: $deleteDialogShown,
            titleVisibility: .visible
        ) {
            Button("Ungroup") {
                model.ungroupTrip(tripID)
                onClose()
            }
            Button("Delete trip & routes", role: .destructive) {
                model.deleteTripAndRoutes(tripID)
                onClose()
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Ungroup keeps the routes in your library. Delete trip & routes removes them too.")
        }
        .confirmationDialog(
            "Remove the last stage?",
            isPresented: dissolveConfirmBinding,
            titleVisibility: .visible,
            presenting: dissolveConfirmStage
        ) { stage in
            Button("Remove & dissolve trip", role: .destructive) {
                _ = model.removeStage(stage, from: tripID)
                onClose()
            }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("The route goes back to your top-level list and the trip is dissolved.")
        }
        // The trip vanished under us (last stage removed, deleted, or every
        // member route deleted elsewhere) — leave the page.
        .onChange(of: model.trips) { _, _ in
            if model.trip(tripID) == nil { onClose() }
        }
        .sheet(item: $tripUploadModel) { model in
            TripUploadSheetView(model: model)
        }
        #if os(iOS)
        .fullScreenCover(isPresented: $mapShown) { tripMapCover }
        #else
        .sheet(isPresented: $mapShown) { tripMapCover }
        #endif
    }

    // MARK: Header

    /// The stages as colored preview tracks — the hero map's and the full-screen
    /// map's shared input (palette color by stage index, the stage rows' rule).
    private var previewStages: [MultiTrackPreviewView.Stage] {
        stages.enumerated().map { index, summary in
            MultiTrackPreviewView.Stage(
                coordinates: summary.trackPreview?.coordinates ?? [],
                color: OBCTheme.stageColor(index: index)
            )
        }
    }

    /// The hero can expand to the interactive map when there's real geometry
    /// and a network path — the route detail's #294 rule, verbatim.
    private var canExpandMap: Bool {
        isOnline && previewStages.contains { !$0.coordinates.isEmpty }
    }

    /// The whole-trip hero map: every stage in its palette color, above the
    /// stat strip. Tapping it (online, with geometry) opens the full-screen
    /// interactive map — the same affordance as the route detail's hero.
    @ViewBuilder
    private var heroMap: some View {
        let preview = MultiTrackPreviewView(stages: previewStages)
            .frame(height: 190)

        if canExpandMap {
            Button { mapShown = true } label: {
                // The preview ignores hits (the tap is ours) — make the whole
                // hero the tap target.
                preview.contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("trip.expandMap")
            .accessibilityLabel("Open full trip map")
        } else {
            preview
        }
    }

    private var tripMapCover: some View {
        // The interactive map draws each stage's FULL tracklog (the #294
        // follow-up rule: never the downsampled preview when zooming is on
        // offer), falling back to the preview's coordinates for a record whose
        // geometry didn't survive. The summaries ride along so a tap on a
        // segment raises its callout; Open route closes the cover and pushes
        // the stage's ordinary detail page.
        TrackMapView(
            stages: stages.enumerated().map { index, summary in
                MultiTrackPreviewView.Stage(
                    coordinates: model.plannedGeometry(for: summary.id)?.points.map(\.coordinate)
                        ?? summary.trackPreview?.coordinates ?? [],
                    color: OBCTheme.stageColor(index: index)
                )
            },
            stageSummaries: stages,
            title: trip?.name ?? "Trip",
            onClose: { mapShown = false },
            onOpenStage: { summary in
                mapShown = false
                onSelectRoute(summary)
            }
        )
    }

    private var header: some View {
        let stats = model.tripStats(tripID)
        return VStack(alignment: .leading, spacing: 14) {
            heroMap

            OBCStatStrip([
                OBCStat(
                    value: OBCFormat.distanceValue(meters: stats.distanceMeters), unit: "km",
                    key: "Distance"),
                OBCStat(
                    value: OBCFormat.climbValue(meters: stats.elevationGainMeters), unit: "m",
                    key: "Climb"),
                OBCStat(value: "\(stats.stageCount)", key: stats.stageCount == 1 ? "Stage" : "Stages"),
            ])
            .accessibilityIdentifier("trip.stats")

            // Primary action (TR8): one tap pushes the whole trip. Link-bound —
            // dims when disconnected; disabled when the trip is already fully up
            // to date on the device (nothing to send). The tap re-reads the
            // device catalogs first (`prepareTripUpload`) so a retry after a
            // failed upload plans against what actually landed — never a
            // duplicate-minting plan cut from a pre-failure cache.
            Button {
                guard !isPreparingUpload else { return }
                isPreparingUpload = true
                Task {
                    tripUploadModel = await model.prepareTripUpload(tripID)
                    isPreparingUpload = false
                }
            } label: {
                Label("Upload trip", systemImage: "square.and.arrow.up")
            }
            .buttonStyle(.obcPrimary)
            .disabled(!canUploadTrip || isPreparingUpload)
            .accessibilityIdentifier("trip.upload")
        }
    }

    /// Upload is offered while connected and the trip isn't already fully current
    /// on the device (an outdated / not-on-device trip has something to push).
    private var canUploadTrip: Bool {
        model.connection == .connected && model.tripOnDeviceState(tripID) != .upToDate
    }

    // MARK: Overflow

    private var overflowMenu: some ToolbarContent {
        ToolbarItem(placement: .primaryAction) {
            Menu {
                Button {
                    renameDraft = trip?.name ?? ""
                    renameShown = true
                } label: { Label("Rename", systemImage: "pencil") }
                .accessibilityIdentifier("trip.rename")

                Button {
                    withAnimation { isReordering.toggle() }
                } label: {
                    Label(
                        isReordering ? "Done reordering" : "Reorder stages",
                        systemImage: "arrow.up.arrow.down")
                }
                .accessibilityIdentifier("trip.reorder")

                Divider()

                Button(role: .destructive) { deleteDialogShown = true } label: {
                    Label("Delete trip…", systemImage: "trash")
                }
                .accessibilityIdentifier("trip.delete")
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .accessibilityIdentifier("trip.overflow")
        }
    }

    // MARK: Edits

    private func moveStages(from source: IndexSet, to destination: Int) {
        model.reorderTripStages(tripID, from: source, to: destination)
    }

    private func deleteStages(at offsets: IndexSet) {
        // Resolve offsets to ids BEFORE mutating: `stages` is computed off the
        // live model, so after the first removal the remaining offsets would
        // point at shifted rows. Each id then goes through the same last-stage
        // guard as the swipe action.
        let ids = offsets.compactMap { $0 < stages.count ? stages[$0].id : nil }
        for id in ids {
            attemptRemove(id)
        }
    }

    /// Remove a stage — directly when the trip keeps at least one, or via the
    /// inline dissolve confirm when it's the last.
    private func attemptRemove(_ routeID: RouteID) {
        if (trip?.stageIDs.count ?? 0) <= 1 {
            dissolveConfirmStage = routeID
        } else {
            _ = model.removeStage(routeID, from: tripID)
        }
    }

    private var dissolveConfirmBinding: Binding<Bool> {
        Binding(
            get: { dissolveConfirmStage != nil },
            set: { if !$0 { dissolveConfirmStage = nil } }
        )
    }
}

extension String {
    /// The string wrapped in typographic double quotes — the dialog-title idiom.
    fileprivate var quoted: String { "\u{201C}\(self)\u{201D}" }
}
