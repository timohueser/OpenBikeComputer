import SwiftUI
import UniformTypeIdentifiers
import OBCDomain
import OBCTransport

/// The hub: device top bar, the "Routes" title with a trailing import button, the Planned and
/// Tracked segments, search, and the compact track-left list. Connection status lives only in the
/// top bar and the disconnected banner. A swipe-left deletes the row directly, because the reveal
/// is the confirm.
///
/// The flows this screen opens stay seams the composition root wires: a card tap, an import pick
/// and settings.
public struct MainScreenView: View {
    @Bindable private var model: MainScreenModel
    private let importFileExtensions: Set<String>
    private let onImportFile: ([URL]) -> Void
    private let onSelectRoute: (RouteSummary) -> Void
    private let onSelectTrip: (Trip) -> Void
    private let onSelectRide: (RideSummary) -> Void
    private let onSettings: () -> Void
    private let onOpenTrash: () -> Void

    @State private var emptyStatePickerShown = false
    @State private var libraryMapShown = false
    // Multi-select grouping: enter Select from the title bar, tap loose route cards, then group
    // them into a trip. Selection is Planned-only, and entering it swaps the card taps for toggles.
    @State private var isSelecting = false
    @State private var selectedRouteIDs: Set<RouteID> = []
    @State private var groupPromptShown = false
    // Pull-to-reveal search, Mail-style: hidden until the list is tugged down past the threshold,
    // and hidden again on scroll-up once the query is cleared. `scrollBaseline` is the sentinel
    // row's resting position.
    @State private var searchRevealed = false
    @State private var scrollBaseline: CGFloat?

    public init(
        model: MainScreenModel,
        importFileExtensions: Set<String> = ["gpx", "tcx"],
        onImportFile: @escaping ([URL]) -> Void = { _ in },
        onSelectRoute: @escaping (RouteSummary) -> Void = { _ in },
        onSelectTrip: @escaping (Trip) -> Void = { _ in },
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
        // `@Bindable` here because `model.sync` is a `let`, which the model-level `@Bindable`
        // cannot project bindings through.
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

            // One banner at a time. A protocol mismatch outranks the rest: the link is up but
            // unusable for data, so it is neither a transfer nor an out-of-range story.
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
                    title: interruption.title,
                    message: interruption.message,
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
                // The bounded ride catalog can report that some rides are outside the returned
                // window, so "up to date" would be a lie. Say so plainly.
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
        .background(OBCTheme.page.ignoresSafeArea())
        // The multi-select action bar, shown only while selecting. Two or more routes make a group.
        .safeAreaInset(edge: .bottom) { selectionBar }
        .obcRenameSheet(
            "Name the trip",
            isPresented: $groupPromptShown,
            name: "New trip",
            placeholder: "Trip name",
            saveTitle: "Create"
        ) {
            model.groupIntoTrip(Array(selectedRouteIDs), name: $0)
            exitSelection()
        }
        #if os(iOS)
        .fullScreenCover(isPresented: $libraryMapShown) { libraryMap }
        #else
        .sheet(isPresented: $libraryMapShown) { libraryMap }
        #endif
        #if os(iOS)
        // The screen draws its own chrome: top bar and large-title row.
        .toolbar(.hidden, for: .navigationBar)
        #endif
        .obcToast(
            isPresented: $sync.upToDateToastVisible,
            message: "You're up to date — no new rides on \(model.deviceName)."
        )
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("main.screen")
        // Selection is a Planned-tab mode: leaving the tab ends it, so the Group bar and Cancel
        // never float over the Tracked list.
        .onChange(of: model.tab) { _, _ in
            if isSelecting { exitSelection() }
        }
        .task { model.start() }
    }

    // MARK: Title actions and selection

    /// The large-title trailing controls: Select and import normally, a single Cancel while
    /// multi-selecting. Select is Planned-only, and hidden with no loose routes to group.
    @ViewBuilder
    private var titleActions: some View {
        if isSelecting {
            Button("Cancel") { exitSelection() }
                .font(.system(.callout, weight: .medium))
                .foregroundStyle(OBCTheme.tint)
                .accessibilityIdentifier("main.selectCancel")
        } else {
            if model.tab == .planned && looseRouteCount > 0 {
                Button("Select") {
                    isSelecting = true
                    selectedRouteIDs = []
                }
                .font(.system(.callout, weight: .medium))
                .foregroundStyle(OBCTheme.tint)
                .accessibilityIdentifier("main.select")
            }
            OBCImportButton(fileExtensions: importFileExtensions, onPick: onImportFile)
        }
    }

    /// The bottom Group bar, present only while selecting.
    @ViewBuilder
    private var selectionBar: some View {
        if isSelecting {
            let count = selectedRouteIDs.count
            Button {
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

    /// Loose, top-level route cards: what Select can group. Trips are not selectable.
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
            .font(.system(.title3, weight: .semibold))
            .foregroundStyle(selected ? OBCTheme.ink : OBCTheme.secondary)
            .padding(8)
            .background(selected ? OBCTheme.surface.opacity(0.9) : .clear, in: Circle())
    }

    // MARK: List

    /// Search stays visible while a query is live, whatever the scroll position, so the query
    /// stays editable.
    private var searchVisible: Bool {
        searchRevealed || !model.searchText.isEmpty
    }

    private var list: some View {
        List {
            // Zero-height sentinel: its offset in the list's space measures top over-scroll. It
            // sits above the search row, so revealing the row does not move its resting position.
            Color.clear
                .frame(height: 0)
                .listRowSeparator(.hidden)
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets())
                .background(
                    GeometryReader { geo in
                        Color.clear
                            // Pin the baseline at rest: waiting for the first `onChange` can
                            // capture it mid-pull, because the sentinel may not move at all until
                            // the first scroll.
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
                    // Transient, Mail-style: once the cleared bar scrolls off the top it
                    // un-reveals. The List culls the row exactly when it leaves the viewport, so
                    // `onDisappear` is the "scrolled away" signal, and the row is off-screen, so
                    // removing it cannot visibly jump. A frame observer cannot do this: it is torn
                    // down in the same cull that would cross the threshold.
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

                // The entry into the trash sits under the Tracked rows, and under the empty state,
                // because deleting the last ride must not strand the trash. It is hidden while a
                // search filters the list, because the row is not a search result.
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
        // The sentinel must truly be 0pt: the List default would give the empty row about 44pt and
        // open a gap under the title.
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
        // Un-revealing is the search row's own job: it fires exactly when the cleared bar scrolls
        // off the top.
    }

    private var tabSelection: Binding<Int> {
        Binding(
            get: { model.tab.rawValue },
            set: { model.tab = MainScreenModel.Tab(rawValue: $0) ?? .planned }
        )
    }

    /// The small stat line under the segments on Tracked: a ride count while syncing, then
    /// the ink confirm.
    @ViewBuilder
    private var syncLine: some View {
        if let progress = model.sync.syncProgress {
            syncLineLabel("\(progress.done) of \(progress.total) rides", color: OBCTheme.secondary, icon: nil)
        } else if let count = model.sync.lastSyncCount {
            syncLineLabel(
                "Synced \(count) new \(count == 1 ? "ride" : "rides") just now",
                color: OBCTheme.ink,
                icon: "checkmark"
            )
        }
    }

    private func syncLineLabel(_ text: String, color: Color, icon: String?) -> some View {
        HStack(spacing: 6) {
            if let icon {
                Image(systemName: icon)
                    .font(.system(.caption2, weight: .bold))
            }
            Text(text)
                .font(.system(.caption).monospacedDigit())
        }
        .foregroundStyle(color)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("main.syncLine")
        .padding(.bottom, 2)
    }

    // MARK: Planned tab

    @ViewBuilder
    private var plannedContent: some View {
        // A trip's routes live only in its line, so a library of trips has no routes.
        if model.loadState == .failed && model.plannedItems.isEmpty {
            readError
        } else if model.loadState == .loading && model.plannedItems.isEmpty {
            skeletons
        } else if model.filteredPlannedItems.isEmpty && !model.searchText.isEmpty {
            noMatches(noun: "routes", scope: "all planned routes")
        } else if model.plannedItems.isEmpty {
            // Empty is not broken: point at the import that fills it.
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
                },
                allowsMultipleSelection: true
            ) { result in
                if case .success(let urls) = result, !urls.isEmpty { onImportFile(urls) }
            }
        } else {
            // Trip cards and route cards, interleaved by `addedAt`. While selecting, route cards
            // toggle instead of navigating and trips dim out: only routes join into a trip.
            ForEach(model.filteredPlannedItems) { item in
                switch item {
                case .trip(let trip):
                    Button {
                        onSelectTrip(trip)
                    } label: {
                        TripCard(
                            name: trip.name,
                            stats: model.tripStats(trip.id),
                            daySummaries: model.tripDays(trip.id).map { $0.summary(tripID: trip.id) },
                            dateLine: model.tripDateLine(trip.id),
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
                            )
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("main.card.\(route.id.rawValue)")
                        .obcSwipeToDelete {
                            model.deleteRoute(route.id)
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
            if model.searchText.isEmpty {
                RideLibraryHeader(model: model.rideLibrary) { libraryMapShown = true }
                    // Keyed on the summaries, not the ids: a trim keeps the id and changes the line.
                    .task(id: model.rides) { await model.rideLibrary.loadMapLines() }
            }
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

    private var libraryMap: some View {
        RideLibraryMapView(
            model: model.rideLibrary,
            onOpenRide: { ride in
                libraryMapShown = false
                onSelectRide(ride)
            },
            onClose: { libraryMapShown = false }
        )
    }

    // MARK: Shared states

    /// Skeletons, not spinners; only an empty first read shimmers.
    private var skeletons: some View {
        ForEach(0..<4, id: \.self) { _ in
            RouteCardSkeleton()
        }
    }

    /// Say what failed, confirm nothing was lost, and offer one retry.
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

    /// Empty results are not an empty library; the query stays editable above.
    private func noMatches(noun: String, scope: String) -> some View {
        VStack(spacing: 6) {
            Image(systemName: "magnifyingglass")
                .font(.system(.largeTitle, weight: .light))
                .foregroundStyle(OBCTheme.secondary)
            Text("No \(noun) match \"\(model.searchText)\"")
                .font(.system(.body, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .multilineTextAlignment(.center)
                .padding(.top, 10)
                .accessibilityIdentifier("main.noMatches")
            Text("Check the spelling, or clear the search to see \(scope).")
                .font(.system(.subheadline))
                .foregroundStyle(OBCTheme.secondary)
                .multilineTextAlignment(.center)
                .lineSpacing(3)
                .frame(maxWidth: 240)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 40)
    }
}

#if DEBUG
#Preview("Main · C1") {
    // Preview-only: a model against a plain placeholder transport is not available here, because
    // OBCUI cannot import OBCMock. The app target's previews drive the full mock-backed screen.
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
    .background(OBCTheme.page)
}
#endif
