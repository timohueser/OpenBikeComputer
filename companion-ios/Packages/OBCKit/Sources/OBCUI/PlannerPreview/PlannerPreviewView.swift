#if os(iOS)
import MapKit
import OBCDomain
import OBCPlanner
import SwiftUI

/// The map-first planner uses the same points, search, and drawer interactions in every build.
public struct PlannerPreviewView: View {
    private enum Panel { case planning, stops, preferences, days, results, place }
    private enum SearchIntent: Equatable { case general, overnight, replace(String) }
    @State private var searchAfterDismissal = false
    @State private var searchQuery = ""
    @State private var model: PlannerPreviewModel
    @ScaledMetric(relativeTo: .body) private var collapsedHeight = PlannerPreviewDrawerPosition.collapsedBase
    @ScaledMetric(relativeTo: .body) private var listContentHeight: CGFloat = 350
    /// Measured from the planning, results and place panels, so the open detent fits its rows.
    @State private var planningContentHeight: CGFloat = 220
    @State private var resultsContentHeight: CGFloat = 300
    @State private var placeContentHeight: CGFloat = 260
    @State private var drawerShown = false
    /// Runs once the drawer has gone: a pop or a save that follows it must not race its dismissal.
    @State private var afterDrawerDismiss: (() -> Void)?
    @State private var drawerPosition = PlannerPreviewDrawerPosition.open
    @State private var editorShown = false
    @State private var layersShown = false
    @State private var infoShown = false
    @State private var attributionShown = false
    @State private var searchShown = false
    @State private var saveShown = false
    @State private var closeShown = false
    @State private var editorPanel = Panel.preferences
    @State private var panel = Panel.planning
    @State private var intent = SearchIntent.general
    @State private var sheetHeight: CGFloat = 340
    @State private var fitRevision = 0
    @State private var fraction: Double?
    @State private var networkStatus: String?
    @State private var results: PlannerPreviewQueryResult?
    @State private var selectedPlace: PlannerPreviewPlace?
    @State private var editingPointID: String?
    @State private var visibleRouteRange: ClosedRange<Double>? = 0...1
    @State private var network = PlannerPreviewNetwork.none
    @State private var hiddenCategories: Set<PlannerPreviewPlaceCategory> = []
    @State private var highlightedCategories: Set<PlannerPreviewPlaceCategory> = []
    @State private var visibleMapRect = MKMapRect.world
    @State private var queryRequest: PlannerPreviewPlaceQuery?
    @State private var queryEditor: PlannerPreviewQueryField?
    @State private var searchSnapshot: (String, PlannerPreviewPlaceQuery?, SearchIntent)?
    @State private var searchSelectedPlace: PlannerPreviewPlace?
    private let onSave: (ImportedRoute, BikeType) -> Void
    private let onClose: () -> Void

    public init(onSave: @escaping (ImportedRoute, BikeType) -> Void,
                onClose: @escaping () -> Void, sample: Bool = false, source: any PlannerDataSource = PlannerService.shared) {
        _model = State(initialValue: PlannerPreviewModel(sample: sample, service: source))
        self.onSave = onSave
        self.onClose = onClose
    }

    public var body: some View {
        GeometryReader { geometry in
            PlannerPreviewMap(coordinates: model.geometry, pins: pins,
                              selectedID: panel == .place ? editingPointID ?? selectedPlace?.id : nil,
                              cursor: cursor, bottomInset: sheetHeight + geometry.safeAreaInsets.bottom,
                              fitRevision: fitRevision, onSelect: selectPin, onMapPoint: selectMapPoint,
                              showCycling: network == .cycling, showHiking: network == .hiking,
                              onVisibleMapRect: { visibleMapRect = $0 },
                              onVisibleRouteRange: { visibleRouteRange = $0 },
                              release: model.release, source: model.service,
                              hiddenCategories: hiddenCategories, highlightedCategories: highlightedCategories,
                              onPlace: selectResult, onNetworkStatus: { networkStatus = $0 })
                .ignoresSafeArea(edges: .bottom)
                .overlay(alignment: .topTrailing) { if !layersShown { mapTools.padding(12) } }
                .overlay(alignment: .topLeading) {
                    if let networkStatus, !layersShown {
                        Text(networkStatus).font(.caption).foregroundStyle(OBCTheme.ink)
                            .padding(8).background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusSmall)).padding(12)
                    } else if attributionShown && !layersShown {
                        Link("© OpenStreetMap", destination: URL(string: "https://www.openstreetmap.org/copyright")!)
                            .font(.caption2).foregroundStyle(OBCTheme.secondary)
                            .padding(.horizontal, 8).frame(minHeight: 44)
                            .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
                            .padding(12)
                    }
                }
                .overlay(alignment: .topTrailing) {
                    if layersShown {
                        PlannerPreviewLayerPanel(network: $network, hidden: $hiddenCategories,
                                                 highlighted: $highlightedCategories) {
                            layersShown = false; drawerPosition = .open
                        }
                        .padding(.top, 16)
                        .frame(width: min(350, geometry.size.width - 24), height: max(240, geometry.size.height - sheetHeight - 32))
                        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
                        .shadow(color: .black.opacity(0.14), radius: 14, y: 4)
                        .padding(12)
                    } else if infoShown {
                        VStack(alignment: .leading, spacing: 12) {
                            HStack {
                                Text("Map data").font(.headline)
                                Spacer()
                                doneButton { infoShown = false }
                            }
                            Link("© OpenStreetMap contributors", destination: URL(string: "https://www.openstreetmap.org/copyright")!)
                            Text("Route networks use the published OpenStreetMap data under the Open Database License.")
                                .font(.footnote).foregroundStyle(OBCTheme.secondary)
                        }
                        .padding(20).frame(width: min(330, geometry.size.width - 24))
                        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
                        .shadow(color: .black.opacity(0.14), radius: 14, y: 4).padding(12)
                    }
                }
                .sheet(isPresented: $drawerShown, onDismiss: { afterDrawerDismiss?(); afterDrawerDismiss = nil }) {
                    PlannerPreviewDrawer(position: $drawerPosition,
                                         openHeight: min(openHeight, geometry.size.height - 80),
                                         expandedHeight: panel == .planning && model.hasRoute
                                            ? min(max(openHeight + 120, geometry.size.height * 0.52), geometry.size.height - 48) : nil,
                                         onHeight: { sheetHeight = $0 }, header: { routeHeader }, content: { drawerContent })
                        .fullScreenCover(isPresented: $searchShown, onDismiss: finishSearch) {
                            PlannerPreviewSearch(model: model, initialQuery: searchQuery, initialRequest: queryRequest,
                                                 initialEditor: queryEditor, onResult: receiveSearch,
                                                 onCancel: cancelSearch,
                                                 onPlace: { searchSelectedPlace = $0; searchShown = false },
                                                 onQueryChange: { searchQuery = $0 },
                                                 onRequestChange: { queryRequest = $0 },
                                                 isInMapView: isInMapView)
                                .withViewBounds(searchBounds)
                        }
                        .sheet(isPresented: $editorShown, onDismiss: finishOpeningSearch) { focusedEditor }
                        .obcRenameSheet("Save route", isPresented: $saveShown, name: model.routeTitle, placeholder: "Route name",
                                        canSave: { model.canSave && !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }) { name in
                            let route = model.exportRoute(name: name), bike = model.bike
                            leave { onSave(route, bike) }
                        }
                        .obcDestructiveConfirm("Discard this route?", isPresented: $closeShown,
                                               message: "Your points and settings go with it.", actionTitle: "Discard") {
                            leave(onClose)
                        }
                }
        }
        .navigationTitle("Plan a route")
        .navigationBarTitleDisplayMode(.inline)
        .navigationBarBackButtonHidden()
        .toolbar(.visible, for: .navigationBar)
        .toolbarBackground(OBCTheme.page, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button {
                    if model.canUndo { closeShown = true } else { leave(onClose) }
                } label: { Label("Library", systemImage: "chevron.left") }
            }
            ToolbarItemGroup(placement: .topBarTrailing) {
                Button { model.undo() } label: { Image(systemName: "arrow.uturn.backward") }
                    .disabled(!model.canUndo).accessibilityLabel("Undo route change")
                    .contextMenu {
                        Button("Redo", systemImage: "arrow.uturn.forward") { model.redo() }.disabled(!model.canRedo)
                    }
                Button("Save", action: beginSave)
                    .fontWeight(.semibold).disabled(!model.canSave).accessibilityIdentifier("planner.save")
            }
        }
        .tint(OBCTheme.tint)
        .task { drawerShown = true }
        .task(id: model.routingRevision) {
            do {
                if model.hasRoute { try await Task.sleep(for: .milliseconds(250)) }
                await model.calculateRoute()
            } catch { }
        }
        .onChange(of: drawerPosition) { _, position in
            if position != .collapsed { layersShown = false; infoShown = false }
        }
        .task(id: network) {
            attributionShown = network != .none
            guard attributionShown else { return }
            do { try await Task.sleep(for: .seconds(5)); attributionShown = false } catch { }
        }
    }

    private var mapTools: some View {
        VStack(spacing: 8) {
            Button { fitRevision += 1; fraction = nil } label: {
                Image(systemName: "arrow.up.left.and.arrow.down.right")
            }.accessibilityLabel("Show whole route")
            Button {
                infoShown = false
                layersShown.toggle()
                if layersShown { drawerPosition = .collapsed }
            } label: { Image(systemName: "square.3.layers.3d") }
                .accessibilityLabel("Map layers")
            if network != .none {
                Button { layersShown = false; infoShown.toggle() } label: {
                    Image(systemName: "info.circle")
                }.accessibilityLabel("Map data attribution")
            }
        }
        .font(.body.weight(.semibold)).buttonStyle(PlannerMapButtonStyle())
    }

    private var routeHeader: some View {
        HStack(spacing: 8) {
            if drawerPosition != .collapsed && panel != .planning {
                if panel == .place, let results, results.action == nil {
                    // The way back to the list this place came from.
                    Button { showResults() } label: {
                        Label(results.title, systemImage: "chevron.left").font(.headline).lineLimit(1)
                            .frame(minHeight: 44).contentShape(Rectangle())
                    }.buttonStyle(.plain).foregroundStyle(OBCTheme.tint).accessibilityIdentifier("planner.backToResults")
                } else {
                    Text(panel == .results ? results?.title ?? "Places" : panel == .place
                         ? (editingPointID != nil ? "Route point" : "Place") : "Route points")
                        .font(.headline).lineLimit(1)
                }
                Spacer(minLength: 4)
                doneButton(action: returnToPlanning)
            } else {
                Button { drawerPosition = .open } label: {
                    VStack(alignment: .leading, spacing: 3) {
                        if drawerPosition != .collapsed || !model.hasRoute {
                            Text(model.hasRoute ? model.routeTitle : (model.start == nil ? "New route" : "Choose a finish"))
                                .font(.headline).lineLimit(1)
                        }
                        if model.isRouting { ProgressView("Calculating route…").font(.caption) }
                        else if model.canSave { statsRow(model.stats) }
                    }.frame(maxWidth: .infinity, minHeight: 44, alignment: .leading).contentShape(Rectangle())
                }.buttonStyle(.plain).accessibilityHint("Show planning controls").accessibilityIdentifier("planner.routeSummary")
                if drawerPosition == .collapsed {
                    Button(action: startSearch) { Image(systemName: "magnifyingglass").frame(width: 44, height: 44).contentShape(Rectangle()) }
                        .accessibilityLabel("Search places").accessibilityIdentifier("planner.quickSearch")
                } else if model.hasRoute {
                    dayChip
                }
            }
        }.foregroundStyle(OBCTheme.ink)
    }

    @ViewBuilder private var drawerContent: some View {
        switch panel {
        case .planning:
            ScrollView {
                VStack(spacing: 10) {
                    if let error = model.routeError {
                        VStack(alignment: .leading, spacing: 6) {
                            Text(error).font(.subheadline).foregroundStyle(OBCTheme.secondary)
                            Button("Try again") { model.retryRoute() }.frame(minHeight: 44)
                        }
                    }
                    if model.canSave { elevation(height: 56 + (drawerPosition == .expanded ? max(0, sheetHeight - openHeight) : 0)) }
                    searchButton
                    OBCGroupedSection {
                        OBCListRow(icon: "point.topleft.down.to.point.bottomright.curvepath", label: "Route points",
                                   value: model.points.isEmpty ? nil : "\(model.points.count)", showsChevron: true) { show(.stops) }
                            .accessibilityIdentifier("planner.points")
                        OBCListRow(icon: "bicycle", label: "Bike", value: model.bike.name, showsChevron: true, showsDivider: false) {
                            show(.preferences)
                        }.accessibilityIdentifier("planner.bike")
                    }
                }.padding(.horizontal, 16).padding(.bottom, 8)
                // The expanded profile must not feed back into the open detent.
                .onGeometryChange(for: CGFloat.self, of: { $0.size.height }) { height in
                    if drawerPosition != .expanded { planningContentHeight = height }
                }
            }.scrollBounceBehavior(.basedOnSize)
        case .results:
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    if let queryRequest {
                        PlannerPreviewQuerySummary(request: queryRequest) { field in
                            resumeSearch(field: field)
                        }
                    }
                    resultContent
                }.padding(.horizontal, 16).padding(.bottom, 16)
                .onGeometryChange(for: CGFloat.self, of: { $0.size.height }) { resultsContentHeight = $0 }
            }
        case .place:
            ScrollView {
                VStack(alignment: .leading, spacing: 14) { placePanel }
                    .padding(.horizontal, 16).padding(.bottom, 16)
                    .onGeometryChange(for: CGFloat.self, of: { $0.size.height }) { placeContentHeight = $0 }
            }.scrollBounceBehavior(.basedOnSize)
        case .stops:
            PlannerPreviewPoints(model: model, onEdit: editPoint, onAdd: startSearch,
                onReverse: { queryRequest = nil; results = model.lookup("reverse"); panel = .results },
                onExample: { model.loadSample(); resetPanel(); fitRevision += 1 },
                onNew: { model.newRoute(); resetPanel(); fitRevision += 1 })
        default: EmptyView()
        }
    }

    private var openHeight: CGFloat {
        let content = switch panel {
        case .planning: planningContentHeight
        case .results: resultsContentHeight
        case .place: placeContentHeight
        default: listContentHeight
        }
        return collapsedHeight + content
    }

    /// The Library's trip badge ("2 days"), made tappable: the door into the day split.
    private var dayChip: some View {
        Button { show(.days) } label: {
            Label(model.dayCount == 1 ? "1 day" : "\(model.dayCount) days", systemImage: model.dayCount == 1 ? "sun.max" : "moon")
                .font(.subheadline.weight(.medium)).foregroundStyle(OBCTheme.ink)
                .padding(.horizontal, 10).frame(minHeight: 32)
                .background(OBCTheme.fill, in: Capsule())
                .frame(minHeight: 44).contentShape(Rectangle())
        }.buttonStyle(.plain).accessibilityIdentifier("planner.days")
    }

    /// Done is quiet text, as on the trip day editor; amber stays for the one action.
    private func doneButton(action: @escaping () -> Void) -> some View {
        Button("Done", action: action)
            .font(.body.weight(.semibold)).foregroundStyle(OBCTheme.tint)
            .frame(minHeight: 44).fixedSize(horizontal: true, vertical: false)
    }

    /// The selected place, route point or map point: its facts, then what to do with it.
    @ViewBuilder private var placePanel: some View {
        if let place = selectedPlace {
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(place.name).font(.system(.title3, weight: .semibold))
                    PlannerOpenBadge(place: place)
                }
                if place.kind != .town {
                    Text(PlannerPlaceRow.detail(for: place, showsRouteDistances: model.hasRoute))
                        .font(.system(.subheadline).monospacedDigit()).foregroundStyle(OBCTheme.secondary)
                }
            }
            if place.hours != nil || place.note != nil {
                OBCGroupedSection {
                    if let hours = place.hours {
                        OBCListRow(icon: "clock", label: "Hours", detail: hours, showsDivider: place.note != nil)
                    }
                    if let note = place.note {
                        OBCListRow(icon: "info.circle", label: note, showsDivider: false)
                    }
                }
            }
            VStack(alignment: .leading, spacing: 4) {
                if editingPointID != nil { pointEditor } else { placeActions(place) }
            }
        }
    }

    private func elevation(height: CGFloat) -> some View {
        PlannerPreviewProfile(profile: model.profile, height: height,
                              visibleRange: visibleRouteRange, selectedFraction: $fraction)
    }

    /// Drawn like `OBCSearchField`, so it reads as the same control as the Library's search.
    private var searchButton: some View {
        Button(action: startSearch) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").font(.subheadline.weight(.semibold))
                Text(PlannerPreviewSearch.prompt(hasRoute: model.hasRoute))
                    .font(.subheadline).frame(maxWidth: .infinity, alignment: .leading)
            }
            .foregroundStyle(OBCTheme.secondary)
            .padding(.horizontal, 12).frame(minHeight: 44)
            .background(OBCTheme.fill, in: RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
            .contentShape(Rectangle())
        }.buttonStyle(.plain).accessibilityIdentifier("planner.search")
    }

    /// The app's self-sizing sheet: the detent fits the content instead of a fixed half screen.
    private var focusedEditor: some View {
        OBCSheetContainer {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .firstTextBaseline) {
                    Text(editorTitle).font(.system(.title3, weight: .semibold)).lineLimit(1)
                    Spacer(minLength: 8)
                    doneButton { editorShown = false }
                }
                if editorPanel == .days { days } else { preferences }
            }.frame(maxWidth: .infinity, alignment: .leading)
        }
        .foregroundStyle(OBCTheme.ink).tint(OBCTheme.tint)
    }

    private var editorTitle: String { editorPanel == .days ? "Days" : "Bike" }

    @ViewBuilder private var pointEditor: some View {
        if let point = (model.points + model.markers).first(where: { $0.id == editingPointID }) {
            // The start and finish are always visits; only the points between them have a kind.
            if point.id != model.points.first?.id, point.id != model.points.last?.id {
                Picker("Point type", selection: Binding(get: { point.kind }, set: { model.setPointKind(id: point.id, kind: $0) })) {
                    ForEach(PlannerPreviewPointKind.allCases, id: \.self) { Text($0.title).tag($0) }
                }.pickerStyle(.segmented)
            }
            Button("Replace place", systemImage: "magnifyingglass") {
                intent = .replace(point.id); searchQuery = ""; openSearchFromDetail()
            }.frame(minHeight: 44)
            if model.points.count > 2, point.id != model.points.first?.id, point.id != model.points.last?.id, point.kind != .marker {
                Button(model.overnightPointID == point.id ? "Remove overnight break" : "End day 1 here", systemImage: "moon") {
                    model.setOvernightPoint(id: model.overnightPointID == point.id ? nil : point.id)
                }.frame(minHeight: 44)
            }
            Button("Remove point", systemImage: "trash", role: .destructive) {
                model.removePoint(id: point.id); resetPanel()
            }.frame(minHeight: 44)
        }
    }

    private var preferences: some View {
        VStack(alignment: .leading, spacing: 14) {
            Picker("Bike", selection: Binding(get: { model.bike }, set: { model.setBike($0) })) {
                ForEach(BikeType.allCases, id: \.self) { Text($0.name).tag($0) }
            }.pickerStyle(.segmented)
            VStack(spacing: 0) {
                ForEach(Array(PlannerPreviewPreset.allCases.enumerated()), id: \.element) { index, preset in
                    if index > 0 { Divider().overlay(OBCTheme.hairline) }
                    Button { model.setPreset(preset) } label: {
                        HStack {
                            Text(preset.title)
                            Spacer()
                            if model.preset == preset { Image(systemName: "checkmark").fontWeight(.semibold) }
                        }.frame(minHeight: 44).contentShape(Rectangle())
                    }.buttonStyle(.plain).disabled(preset == .smoother)
                }
                Text("Smoother is not available from the route service.").font(.footnote).foregroundStyle(OBCTheme.secondary)
            }
        }
    }

    private var days: some View {
        VStack(alignment: .leading, spacing: 16) {
            if !model.hasRoute {
                Text("Choose your start and finish first.").foregroundStyle(OBCTheme.secondary)
            } else if let overnight = model.overnight {
                ForEach(Array(model.dayStats.enumerated()), id: \.offset) { index, stats in
                    VStack(alignment: .leading, spacing: 6) {
                        Text("Day \(index + 1)").font(.headline)
                        Text(index == 0 ? "\(model.start!.name) → \(overnight.name)" : "\(overnight.name) → \(model.finish!.name)")
                            .font(.subheadline)
                        statsRow(stats)
                    }.padding(.vertical, 4)
                    Divider()
                }
                Button("Change overnight stop", systemImage: "tent") {
                    searchQuery = "camping"; intent = .overnight; openSearchFromDetail()
                }
                    .frame(minHeight: 44)
                Button("Make it a single day") { model.setOvernight(nil) }.buttonStyle(.obcGhost)
            } else {
                Button("Add an overnight stop", systemImage: "tent") {
                    searchQuery = "camping"; queryRequest = PlannerPreviewPlaceQuery.parse("camping", hasRoute: model.hasRoute)
                    queryEditor = nil; intent = .overnight
                    openSearchFromDetail()
                }.buttonStyle(.obcPrimary)
            }
        }
    }

    @ViewBuilder private var resultContent: some View {
        if let results {
            if let action = results.action {
                Text(model.actionSummary(action))
                Button(action.title) { model.apply(action); resetPanel() }.buttonStyle(.obcPrimary)
            } else if results.places.isEmpty {
                Text("No places match these filters").foregroundStyle(OBCTheme.secondary)
                Button("Try another search") { resumeSearch() }.buttonStyle(.obcGhost)
            } else {
                ForEach(results.places) { place in
                    Button { selectResult(place) } label: { PlannerPlaceRow(place: place, showsRouteDistances: model.hasRoute) }
                        .buttonStyle(.plain)
                    Divider().overlay(OBCTheme.hairline)
                }
            }
        }
    }

    @ViewBuilder private func placeActions(_ place: PlannerPreviewPlace) -> some View {
        if let existing = (model.points + model.markers).first(where: { $0.place.id == place.id }) {
            Button("Edit existing point") { editPoint(existing) }.buttonStyle(.obcPrimary)
        } else if case .replace(let id) = intent {
            Button("Use this place") { model.replacePoint(id: id, with: place); finishIntent() }.buttonStyle(.obcPrimary)
        } else if intent == .overnight {
            Button("End day 1 here", systemImage: "moon") { model.setOvernight(place); finishIntent() }.buttonStyle(.obcPrimary)
        } else if model.start == nil {
            Button("Start here") { model.setStart(place); resetPanel() }.buttonStyle(.obcPrimary)
        } else if model.finish == nil {
            Button("Finish here") { model.setFinish(place); resetPanel(); fitRevision += 1 }.buttonStyle(.obcPrimary)
        } else {
            // Most taps mean "stop here"; the other point kinds wait behind More.
            Button("Add stop") { model.addPoint(place, kind: .visit); resetPanel() }
                .buttonStyle(.obcPrimary).accessibilityIdentifier("planner.addStop")
            Menu {
                Button("Route through here, no stop", systemImage: PlannerPreviewPointKind.shape.symbol) {
                    model.addPoint(place, kind: .shape); resetPanel()
                }
                Button("Mark on the map only", systemImage: PlannerPreviewPointKind.marker.symbol) {
                    model.addPoint(place, kind: .marker); resetPanel()
                }
                if place.kind == .camping {
                    Button("End day 1 here", systemImage: "moon") { model.setOvernight(place); resetPanel() }
                }
            } label: {
                Text("More").font(.subheadline.weight(.semibold)).foregroundStyle(OBCTheme.tint)
                    .frame(maxWidth: .infinity, minHeight: 44).contentShape(Rectangle())
            }.accessibilityIdentifier("planner.morePointKinds")
        }
    }

    private func editPoint(_ point: PlannerPreviewPoint) {
        editingPointID = point.id; selectedPlace = point.place; intent = .general
        panel = .place; drawerPosition = .open
    }

    /// The Library's stat line, so the planner and the list agree on every figure.
    private func statsRow(_ stats: PlannerPreviewStats) -> some View {
        let distance = Text(OBCFormat.distance(meters: stats.distanceMeters))
        let ascent = Text.obcClimb(meters: stats.ascentMeters)
        let duration = Text(OBCFormat.estimatedClock(stats.seconds))
        return ViewThatFits(in: .horizontal) {
            HStack(spacing: 14) { distance; ascent; duration }.fixedSize(horizontal: true, vertical: false)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 14) { distance; duration }
                ascent
            }
        }.font(.subheadline.monospacedDigit()).foregroundStyle(OBCTheme.secondary)
    }

    private func startSearch() {
        searchSnapshot = nil
        layersShown = false; infoShown = false
        intent = .general; searchQuery = ""; queryRequest = nil; queryEditor = nil; searchShown = true
    }

    private func resumeSearch(field: PlannerPreviewQueryField? = nil) {
        searchSnapshot = (searchQuery, queryRequest, intent)
        queryEditor = field; searchShown = true
    }

    private func cancelSearch() {
        if let snapshot = searchSnapshot {
            searchQuery = snapshot.0; queryRequest = snapshot.1; intent = snapshot.2
        } else { intent = .general }
        searchSnapshot = nil; searchShown = false
    }

    private func openSearchFromDetail() {
        searchSnapshot = nil
        queryRequest = nil; queryEditor = nil
        if editorShown { searchAfterDismissal = true; editorShown = false }
        else { searchShown = true }
    }

    private func finishOpeningSearch() {
        guard searchAfterDismissal else { return }
        searchAfterDismissal = false; searchShown = true
    }

    private func finishSearch() {
        queryEditor = nil; searchSnapshot = nil
        if let place = searchSelectedPlace {
            searchSelectedPlace = nil; selectResult(place)
        }
    }

    private func beginSave() {
        guard model.canSave else { return }
        saveShown = true
    }

    private func show(_ panel: Panel) {
        layersShown = false; infoShown = false
        if panel == .stops || panel == .results { self.panel = panel; drawerPosition = .open }
        else { editorPanel = panel; editorShown = true }
    }

    private func returnToPlanning() {
        panel = .planning; drawerPosition = .open; results = nil; fraction = nil; intent = .general
        selectedPlace = nil; editingPointID = nil
    }

    private func showResults() {
        selectedPlace = nil; editingPointID = nil
        panel = .results; drawerPosition = .open
    }

    /// After an edit: back to the list the place came from, else to planning. The camera stays.
    private func resetPanel() {
        if let results, results.action == nil { showResults() } else { queryRequest = nil; returnToPlanning() }
    }

    /// A replace or an overnight pick answers the search that asked for it, so its list closes.
    private func finishIntent() {
        queryRequest = nil; returnToPlanning()
    }

    private func receiveSearch(_ result: PlannerPreviewQueryResult) {
        model.mapPlaces = result.places
        results = result; selectedPlace = nil; editingPointID = nil
        panel = .results; drawerPosition = .open; searchShown = false
        if !result.places.isEmpty { fitRevision += 1 }
    }

    private func selectResult(_ place: PlannerPreviewPlace) {
        selectedPlace = place; editingPointID = nil
        panel = .place; drawerPosition = .open
    }

    // A tap on the map always selects what was tapped; the place panel just shows the next one.
    private func selectPin(_ id: String, at location: CGPoint) {
        layersShown = false; infoShown = false
        if let point = (model.points + model.markers).first(where: { $0.id == id }) {
            editPoint(point)
        } else if let place = ((results?.places ?? []) + model.mapPlaces).first(where: { $0.id == id }) {
            selectResult(place)
        }
    }

    private func selectMapPoint(_ coordinate: Coordinate, at location: CGPoint) {
        editingPointID = nil; layersShown = false; infoShown = false
        selectedPlace = .init(id: UUID().uuidString, name: "Map point", coordinate: coordinate)
        panel = .place; drawerPosition = .open
    }

    /// Closes the drawer first and runs `action` after it has gone.
    private func leave(_ action: @escaping () -> Void) {
        afterDrawerDismiss = action
        drawerShown = false
    }

    private func isInMapView(_ place: PlannerPreviewPlace) -> Bool {
        visibleMapRect.contains(MKMapPoint(CLLocationCoordinate2D(latitude: place.coordinate.latitude, longitude: place.coordinate.longitude)))
    }

    private var searchBounds: [Double]? {
        guard visibleMapRect.size.width < MKMapSize.world.width else { return model.release?.bounds }
        let nw = MKMapPoint(x: visibleMapRect.minX, y: visibleMapRect.minY).coordinate
        let se = MKMapPoint(x: visibleMapRect.maxX, y: visibleMapRect.maxY).coordinate
        return [nw.longitude, se.latitude, se.longitude, nw.latitude]
    }

    private var pins: [PlannerPreviewMapPin] {
        var pins = model.points.enumerated().map { index, point in
            PlannerPreviewMapPin(id: point.id, title: point.place.name, coordinate: point.place.coordinate,
                symbol: point.id == model.overnightPointID ? "moon.fill" : point.place.kind.symbol,
                kind: index == 0 ? .start : index == model.points.count - 1 ? .finish : point.kind == .shape ? .shape : .stop)
        }
        pins += model.markers.map { .init(id: $0.id, title: $0.place.name, coordinate: $0.place.coordinate, symbol: "mappin", kind: .marker) }
        var places = results?.places ?? []
        places += model.mapPlaces.filter { place in
            guard let category = PlannerPreviewPlaceCategory.category(for: place) else { return model.points.isEmpty }
            return results == nil && !hiddenCategories.contains(category)
        }
        if panel == .place, let selectedPlace, editingPointID == nil { places.insert(selectedPlace, at: 0) }
        var seen = Set((model.points + model.markers).map { $0.place.id })
        pins += places.filter { seen.insert($0.id).inserted }.map { place in
            .init(id: place.id, title: place.name, coordinate: place.coordinate, symbol: place.kind.symbol,
                  highlighted: results?.places.contains(where: { $0.id == place.id }) == true
                    || (PlannerPreviewPlaceCategory.category(for: place).map { highlightedCategories.contains($0) } ?? false),
                  isAmbient: place.kind != .town)
        }
        return pins
    }
    private var cursor: Coordinate? {
        guard let fraction, model.geometry.count > 1 else { return nil }
        let line = model.routeLine
        return line.coordinate(at: fraction * line.length)
    }
}

private struct PlannerMapButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var enabled
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.frame(width: 44, height: 44)
            .foregroundStyle(enabled ? OBCTheme.ink : OBCTheme.secondary.opacity(0.45))
            .background(configuration.isPressed ? OBCTheme.page : OBCTheme.surface,
                        in: RoundedRectangle(cornerRadius: OBCTheme.controlRadius))
            .shadow(color: .black.opacity(0.12), radius: 5, y: 2)
    }
}
#endif
