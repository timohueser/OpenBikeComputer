import SwiftUI
import UniformTypeIdentifiers
import OBCDomain
import OBCTransport

/// The hub (B3, design C1/C2): device top bar, serif "Routes" title with the
/// trailing **+ import**, Planned | Tracked segments, search, and the compact
/// track-left list. Connection status lives *only* in the top bar + the S4
/// banner; swipe-left deletes the row directly (the reveal is the confirm —
/// one-tap detail deletes still go through H1).
///
/// Navigation and the flows this screen *opens* stay seams the composition
/// root wires: card tap → route detail (B4), import pick (B6), settings (B8).
public struct MainScreenView: View {
    @Bindable private var model: MainScreenModel
    private let importFileExtensions: Set<String>
    private let onImportFile: (URL) -> Void
    private let onSelectRoute: (RouteSummary) -> Void
    private let onSelectTrip: (TripRecord) -> Void
    private let onSelectRide: (RideSummary) -> Void
    private let onSettings: () -> Void
    private let onOpenTrash: () -> Void

    @State private var emptyStatePickerShown = false
    // Multi-select grouping (TR7): the primary retrofit path — enter Select from
    // the title bar, tap loose route cards, then Group into trip…. Selection is
    // Planned-only; entering it swaps the card taps for toggles and shows the
    // bottom action bar.
    @State private var isSelecting = false
    @State private var selectedRouteIDs: Set<RouteID> = []
    @State private var groupPromptShown = false
    @State private var groupName = "New trip"
    /// The loose route whose "Add to trip…" context menu is opening the shared
    /// picker (TR7) — `nil` when no picker is up.
    @State private var pickerRequest: RouteTripPickerRequest?
    // Pull-to-reveal search (Mail-style): hidden until the list is tugged
    // down past the threshold; hides again on scroll-up once the query is
    // cleared. `scrollBaseline` is the sentinel row's resting position.
    @State private var searchRevealed = false
    @State private var scrollBaseline: CGFloat?

    public init(
        model: MainScreenModel,
        importFileExtensions: Set<String> = ["gpx", "tcx"],
        onImportFile: @escaping (URL) -> Void = { _ in },
        onSelectRoute: @escaping (RouteSummary) -> Void = { _ in },
        onSelectTrip: @escaping (TripRecord) -> Void = { _ in },
        onSelectRide: @escaping (RideSummary) -> Void = { _ in },
        onSettings: @escaping () -> Void = {},
        onOpenTrash: @escaping () -> Void = {}
    ) {
        self.model = model
        self.importFileExtensions = importFileExtensions
        self.onImportFile = onImportFile
        self.onSelectRoute = onSelectRoute
        self.onSelectTrip = onSelectTrip
        self.onSelectRide = onSelectRide
        self.onSettings = onSettings
        self.onOpenTrash = onOpenTrash
    }

    public var body: some View {
        // The sync state machine, read straight off the model's coordinator
        // (#358) — `@Bindable` here because `model.sync` is a `let` the
        // model-level `@Bindable` can't project bindings through.
        @Bindable var sync = model.sync
        VStack(spacing: 0) {
            DeviceTopBar(
                deviceName: model.deviceName,
                connection: model.connection,
                batteryPercent: model.battery,
                syncState: sync.syncState,
                onSync: { sync.sync() },
                onSettings: onSettings
            )

            // One banner at a time. A protocol mismatch (#303) outranks the rest:
            // the link is up but unusable for data, so it can't be a transfer or
            // an out-of-range story — sync is disabled until the versions match.
            if let mismatch = model.protocolMismatch {
                OBCInlineBanner(
                    tone: .warning,
                    systemImage: "exclamationmark.triangle",
                    title: "Can't sync with \(model.deviceName).",
                    message: mismatch.found > mismatch.expected
                        ? "Update the app to match this OBC."
                        : "Update the OBC to match this app."
                )
                .accessibilityIdentifier("protocolMismatchBanner")
                .padding(.horizontal, 20)
                .padding(.bottom, 6)
            } else if let interruption = sync.syncInterruption {
                OBCInlineBanner(
                    tone: .warning,
                    systemImage: "exclamationmark.triangle",
                    title: "Sync interrupted.",
                    message: "Got \(interruption.landed) of \(interruption.total) rides.",
                    actionTitle: "Resume",
                    action: { sync.resumeSync() }
                )
                .accessibilityIdentifier("syncInterruptedBanner")
                .padding(.horizontal, 20)
                .padding(.bottom, 6)
            } else if model.showsDisconnectedBanner {
                OBCInlineBanner(
                    systemImage: "wifi.slash",
                    title: "\(model.deviceName) is out of range.",
                    message: "Showing your last sync."
                )
                .accessibilityIdentifier("disconnectedBanner")
                .padding(.horizontal, 20)
                .padding(.bottom, 6)
            } else if sync.hiddenRideCount > 0 {
                // The bounded ride catalog can report that some
                // rides are dropped from the list in FAT-arbitrary order, so
                // "up to date" would be a lie — say so plainly (iOS tone rule).
                OBCInlineBanner(
                    systemImage: "externaldrive.badge.exclamationmark",
                    title: sync.hiddenRideCount == 1
                        ? "1 ride on \(model.deviceName) can't be listed."
                        : "\(sync.hiddenRideCount) rides on \(model.deviceName) can't be listed.",
                    message: "Free up space on the device to sync them."
                )
                .accessibilityIdentifier("ridesTruncatedBanner")
                .padding(.horizontal, 20)
                .padding(.bottom, 6)
            }

            OBCLargeTitleBar("Routes") {
                titleActions
            }

            list
        }
        .background(OBCTheme.parchment.ignoresSafeArea())
        // The multi-select action bar (TR7): a bottom Group into trip… primary,
        // shown only while selecting. Two or more routes make a group.
        .safeAreaInset(edge: .bottom) { selectionBar }
        // The name prompt — prefilled "New trip" (nothing fancier, locked).
        .alert("Name the trip", isPresented: $groupPromptShown) {
            TextField("Trip name", text: $groupName)
            Button("Cancel", role: .cancel) {}
            Button("Create") {
                model.groupIntoTrip(Array(selectedRouteIDs), name: groupName)
                exitSelection()
            }
        }
        // The shared trip picker for a loose route's "Add to trip…" context menu.
        .sheet(item: $pickerRequest) { request in
            TripPickerSheet(
                title: "Add to trip",
                trips: model.tripPickerItems,
                onPick: { model.fileRoute(request.id, into: $0) }
            )
        }
        #if os(iOS)
        // The screen draws its own chrome (top bar + large-title row).
        .toolbar(.hidden, for: .navigationBar)
        #endif
        .obcToast(
            isPresented: $sync.upToDateToastVisible,
            message: "You're up to date — no new rides on \(model.deviceName)."
        )
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("main.screen")
        // Selection is a Planned-tab mode: leaving the tab ends it, so the
        // Group bar and Cancel never float over the Tracked list.
        .onChange(of: model.tab) { _, _ in
            if isSelecting { exitSelection() }
        }
        .task { model.start() }
    }

    // MARK: Title actions + selection (TR7)

    /// The large-title trailing controls: Select + import normally; a single
    /// Cancel while multi-selecting (import is out of the way mid-group). Select
    /// is Planned-only and hidden with no loose routes to group.
    @ViewBuilder
    private var titleActions: some View {
        if isSelecting {
            Button("Cancel") { exitSelection() }
                .font(.system(size: 16, weight: .medium))
                .foregroundStyle(OBCTheme.tint)
                .accessibilityIdentifier("main.selectCancel")
        } else {
            if model.tab == .planned && looseRouteCount > 0 {
                Button("Select") {
                    isSelecting = true
                    selectedRouteIDs = []
                }
                .font(.system(size: 16, weight: .medium))
                .foregroundStyle(OBCTheme.tint)
                .accessibilityIdentifier("main.select")
            }
            OBCImportButton(fileExtensions: importFileExtensions, onPick: onImportFile)
        }
    }

    /// The bottom Group into trip… bar, present only while selecting.
    @ViewBuilder
    private var selectionBar: some View {
        if isSelecting {
            let count = selectedRouteIDs.count
            Button {
                groupName = "New trip"
                groupPromptShown = true
            } label: {
                Text(count > 0 ? "Group into trip (\(count))" : "Group into trip")
            }
            .buttonStyle(.obcPrimary)
            .disabled(count < 2)
            .accessibilityIdentifier("main.groupIntoTrip")
            .padding(.horizontal, 20)
            .padding(.top, 10)
            .padding(.bottom, 8)
            .background(.ultraThinMaterial)
        }
    }

    /// Loose (top-level) route cards — what Select can group. Trips aren't
    /// selectable.
    private var looseRouteCount: Int {
        model.plannedItems.reduce(0) { count, item in
            if case .route = item { return count + 1 }
            return count
        }
    }

    private func toggleSelection(_ id: RouteID) {
        if selectedRouteIDs.contains(id) {
            selectedRouteIDs.remove(id)
        } else {
            selectedRouteIDs.insert(id)
        }
    }

    private func exitSelection() {
        isSelecting = false
        selectedRouteIDs = []
    }

    /// The selection tick over a route card while grouping.
    private func selectionCheck(on id: RouteID) -> some View {
        let selected = selectedRouteIDs.contains(id)
        return Image(systemName: selected ? "checkmark.circle.fill" : "circle")
            .font(.system(size: 20, weight: .semibold))
            .foregroundStyle(selected ? OBCTheme.forest : OBCTheme.inkFaint)
            .padding(8)
            .background(selected ? OBCTheme.panel.opacity(0.9) : .clear, in: Circle())
    }

    // MARK: List

    /// Search stays visible while a query is live regardless of scroll (H6
    /// keeps the query editable).
    private var searchVisible: Bool {
        searchRevealed || !model.searchText.isEmpty
    }

    private var list: some View {
        List {
            // Zero-height sentinel: its offset in the list's space measures
            // top over-scroll. It sits above the search row, so revealing the
            // row doesn't move the sentinel's resting position.
            Color.clear
                .frame(height: 0)
                .listRowSeparator(.hidden)
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets())
                .background(
                    GeometryReader { geo in
                        Color.clear
                            // Pin the baseline at rest — waiting for the first
                            // onChange can capture it mid-pull (the sentinel
                            // may not move at all until the first scroll).
                            .onAppear {
                                if scrollBaseline == nil {
                                    scrollBaseline = geo.frame(in: .named("mainList")).minY
                                }
                            }
                            .onChange(of: geo.frame(in: .named("mainList")).minY) { _, minY in
                                handleTopOverscroll(minY)
                            }
                    }
                )

            Group {
                OBCSegmentedControl(selection: tabSelection, labels: ["Planned", "Tracked"])
                    .padding(.top, 4)
                    .padding(.bottom, 2)

                if searchVisible {
                    OBCSearchField(
                        text: $model.searchText,
                        prompt: model.tab == .planned ? "Search routes" : "Search rides"
                    )
                    .accessibilityIdentifier("main.search")
                    // Transient, Mail-style: once the (cleared) bar scrolls off
                    // the top it un-reveals. The List culls the row exactly when
                    // it leaves the viewport, so `onDisappear` IS the "scrolled
                    // away" signal — and the row is off-screen, so removing it
                    // can't visibly jump. (A frame observer can't do this: it's
                    // torn down in the same cull that would cross the threshold.)
                    .onDisappear {
                        if model.searchText.isEmpty { searchRevealed = false }
                    }
                }

                if model.tab == .tracked {
                    syncLine
                }

                switch model.tab {
                case .planned: plannedContent
                case .tracked: trackedContent
                }

                // Recently Deleted (#292): the entry into the trash sits under
                // the Tracked rows (and under the empty state — deleting the
                // last ride must not strand the trash). Hidden while a search
                // filters the list: the row isn't a search result.
                if model.tab == .tracked, !model.trashedRides.isEmpty, model.searchText.isEmpty {
                    OBCDisclosureRow(
                        systemImage: "trash",
                        label: "Recently Deleted",
                        value: "\(model.trashedRides.count)",
                        accessibilityID: "main.recentlyDeleted",
                        action: onOpenTrash
                    )
                    .padding(.top, 8)
                }
            }
            .listRowSeparator(.hidden)
            .listRowBackground(Color.clear)
            .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 12, trailing: 20))
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        // The sentinel must truly be 0pt — the List default would give the
        // empty row ~44pt and open a gap under the title.
        .environment(\.defaultMinListRowHeight, 0)
        .coordinateSpace(name: "mainList")
        #if os(iOS)
        .scrollDismissesKeyboard(.immediately)
        #endif
    }

    private func handleTopOverscroll(_ minY: CGFloat) {
        guard let baseline = scrollBaseline else {
            scrollBaseline = minY
            return
        }
        if minY - baseline > 55, !searchRevealed {
            withAnimation(.easeOut(duration: 0.2)) { searchRevealed = true }
        }
        // Un-revealing is the search row's own job (frame observer above):
        // it fires exactly when the cleared bar scrolls off the top.
    }

    private var tabSelection: Binding<Int> {
        Binding(
            get: { model.tab.rawValue },
            set: { model.tab = MainScreenModel.Tab(rawValue: $0) ?? .planned }
        )
    }

    /// The small mono line under the segments on Tracked (C2): amber ride-count
    /// while syncing, the forest "Synced N new rides just now" confirm after.
    @ViewBuilder
    private var syncLine: some View {
        if let progress = model.sync.syncProgress {
            syncLineLabel("\(progress.done) of \(progress.total) rides", color: OBCTheme.amber, icon: nil)
        } else if let count = model.sync.lastSyncCount {
            syncLineLabel(
                "Synced \(count) new \(count == 1 ? "ride" : "rides") just now",
                color: OBCTheme.forest,
                icon: "checkmark"
            )
        }
    }

    private func syncLineLabel(_ text: String, color: Color, icon: String?) -> some View {
        HStack(spacing: 6) {
            if let icon {
                Image(systemName: icon)
                    .font(.system(size: 11, weight: .bold))
            }
            Text(text)
                .font(.obcMono(size: 12))
        }
        .foregroundStyle(color)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("main.syncLine")
        .padding(.bottom, 2)
    }

    // MARK: Planned tab

    @ViewBuilder
    private var plannedContent: some View {
        if model.loadState == .failed && model.routes.isEmpty {
            readError
        } else if model.loadState == .loading && model.routes.isEmpty {
            skeletons
        } else if model.filteredPlannedItems.isEmpty && !model.searchText.isEmpty {
            noMatches(noun: "routes", scope: "all planned routes")
        } else if model.routes.isEmpty {
            // S1 — empty ≠ broken: point at the import that fills it (B10 owns
            // the app-state pass; the copy is the design's).
            OBCEmptyStateView(
                glyph: .trackTile,
                title: "No planned routes yet",
                message: "Tap + to import a .gpx from Files, or share one from Komoot, Strava, or any app.",
                actionTitle: "Import a route",
                actionSystemImage: "plus"
            ) {
                emptyStatePickerShown = true
            }
            .padding(.top, 40)
            .fileImporter(
                isPresented: $emptyStatePickerShown,
                allowedContentTypes: importFileExtensions.sorted().compactMap {
                    UTType(filenameExtension: $0)
                }
            ) { result in
                if case .success(let url) = result { onImportFile(url) }
            }
        } else {
            // TR6: trip cards + loose route cards, interleaved by addedAt.
            // TR7: while selecting, route cards toggle instead of navigating and
            // trips dim out (a trip isn't a groupable stage).
            ForEach(model.filteredPlannedItems) { item in
                switch item {
                case .trip(let trip):
                    Button {
                        onSelectTrip(trip)
                    } label: {
                        TripCard(
                            name: trip.name,
                            stats: model.tripStats(trip.id),
                            stageSummaries: model.tripStages(trip.id),
                            onDevice: model.tripOnDeviceState(trip.id)
                        )
                    }
                    .buttonStyle(.plain)
                    .disabled(isSelecting)
                    .opacity(isSelecting ? 0.4 : 1)
                    .accessibilityIdentifier("main.trip.\(trip.id.rawValue)")
                case .route(let route, _):
                    if isSelecting {
                        Button {
                            toggleSelection(route.id)
                        } label: {
                            RouteCard(
                                route: route,
                                onDevice: model.onDeviceState(route.id),
                                expiryBadge: model.expiryBadge(for: route.id)
                            )
                                .overlay(alignment: .topTrailing) {
                                    selectionCheck(on: route.id)
                                }
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("main.card.\(route.id.rawValue)")
                        .accessibilityAddTraits(
                            selectedRouteIDs.contains(route.id) ? .isSelected : [])
                    } else {
                        Button {
                            onSelectRoute(route)
                        } label: {
                            RouteCard(
                                route: route,
                                onDevice: model.onDeviceState(route.id),
                                expiryBadge: model.expiryBadge(for: route.id)
                            )
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("main.card.\(route.id.rawValue)")
                        .obcSwipeToDelete {
                            model.deleteRoute(route.id)
                        }
                        // Clip the long-press lift preview to the card's own
                        // rounded shape — without this the system snapshots the
                        // whole rectangular row and the card floats on a stark
                        // white slab. (iOS-only kind; macOS is the test host.)
                        #if os(iOS)
                        .contentShape(
                            .contextMenuPreview,
                            RoundedRectangle(cornerRadius: OBCTheme.radiusCard)
                        )
                        #endif
                        .contextMenu {
                            Button {
                                pickerRequest = RouteTripPickerRequest(id: route.id)
                            } label: {
                                Label("Add to trip…", systemImage: "folder.badge.plus")
                            }
                        }
                    }
                }
            }
        }
    }

    // MARK: Tracked tab

    @ViewBuilder
    private var trackedContent: some View {
        if model.loadState == .failed && model.rides.isEmpty {
            readError
        } else if model.loadState == .loading && model.rides.isEmpty {
            skeletons
        } else if model.filteredRides.isEmpty && !model.searchText.isEmpty {
            noMatches(noun: "rides", scope: "all tracked rides")
        } else if model.rides.isEmpty {
            OBCEmptyStateView(
                glyph: .trackTile,
                title: "No rides yet",
                message: "Rides you record on \(model.deviceName) land here after a sync."
            )
            .padding(.top, 40)
        } else {
            ForEach(model.filteredRides) { ride in
                Button {
                    onSelectRide(ride)
                } label: {
                    RouteCard(ride: ride)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("main.card.\(ride.id.rawValue)")
                .obcSwipeToDelete {
                    model.deleteRide(ride.id)
                }
            }
        }
    }

    // MARK: Shared states

    /// S2 — skeletons, not spinners; only an empty first read shimmers.
    private var skeletons: some View {
        ForEach(0..<4, id: \.self) { _ in
            RouteCardSkeleton()
        }
    }

    /// S3 — say what failed, confirm nothing was lost, offer one retry.
    private var readError: some View {
        OBCEmptyStateView(
            glyph: .warning(systemImage: "exclamationmark.triangle"),
            title: "Couldn't read \(model.deviceName)",
            message: "The connection dropped mid-read. Your saved routes are still here.",
            actionTitle: "Retry"
        ) {
            model.reload()
        }
        .padding(.top, 40)
        .accessibilityIdentifier("main.readError")
    }

    /// H6 — empty results ≠ empty library; the query stays editable above.
    private func noMatches(noun: String, scope: String) -> some View {
        VStack(spacing: 6) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 34, weight: .light))
                .foregroundStyle(OBCTheme.inkFaint)
            Text("No \(noun) match \"\(model.searchText)\"")
                .font(.obcSerif(size: 18))
                .foregroundStyle(OBCTheme.ink)
                .multilineTextAlignment(.center)
                .padding(.top, 10)
                .accessibilityIdentifier("main.noMatches")
            Text("Check the spelling, or clear the search to see \(scope).")
                .font(.system(size: 14))
                .foregroundStyle(OBCTheme.inkSoft)
                .multilineTextAlignment(.center)
                .lineSpacing(3)
                .frame(maxWidth: 240)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 40)
    }
}

/// A loose route whose "Add to trip…" context menu is presenting the shared
/// picker (TR7) — the `Identifiable` handle a `.sheet(item:)` needs.
private struct RouteTripPickerRequest: Identifiable {
    let id: RouteID
}

#if DEBUG
#Preview("Main · C1") {
    // Preview-only: a model against a plain placeholder transport is not
    // available here (OBCUI can't import OBCMock) — see the app target's
    // RootView previews for the full mock-driven screen.
    VStack(spacing: 0) {
        DeviceTopBar(deviceName: "Trailhead", connection: .connected, batteryPercent: 82)
        OBCLargeTitleBar("Routes") {
            OBCImportButton(fileExtensions: ["gpx", "tcx"]) { _ in }
        }
        ScrollView {
            VStack(spacing: 12) {
                RouteCard(title: "Kettle Moraine Loop", subtitle: "62.4 km · 840 m ↑ · 3h 20m", preview: .obcSample)
                RouteCard(title: "Sugar River Trail", subtitle: "38.1 km · 210 m ↑ · 1h 55m", preview: .obcSample)
            }
            .padding(.horizontal, 20)
        }
    }
    .background(OBCTheme.parchment)
}
#endif
