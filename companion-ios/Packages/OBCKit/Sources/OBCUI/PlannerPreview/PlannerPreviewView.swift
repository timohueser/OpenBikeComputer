#if DEBUG && os(iOS)
import MapKit
import OBCDomain
import SwiftUI

/// A development-only native study of the map-first planning flow.
public struct PlannerPreviewView: View {
    private enum Panel { case planning, stops, preferences, days, results, point }
    private enum SearchIntent: Equatable { case general, replace(String) }
    @State private var searchAfterDismissal = false
    @State private var searchQuery = ""
    @State private var model: PlannerPreviewModel
    @ScaledMetric(relativeTo: .body) private var planningHeight: CGFloat = 276
    @ScaledMetric(relativeTo: .body) private var listHeight: CGFloat = 410
    @State private var drawerShown = false
    @State private var drawerPosition = PlannerPreviewDrawerPosition.open
    @State private var editorShown = false
    @State private var layersShown = false
    @State private var infoShown = false
    @State private var attributionShown = false
    @State private var mapPlaceShown = false
    @State private var mapAnchor = CGPoint.zero
    @State private var searchShown = false
    @State private var saveShown = false
    @State private var closeShown = false
    @State private var editorPanel = Panel.point
    @State private var panel = Panel.planning
    @State private var intent = SearchIntent.general
    @State private var sheetHeight: CGFloat = 340
    @State private var fitRevision = 0
    @State private var fraction: Double?
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
    @State private var mapContextHeight: CGFloat = 280
    @State private var routeName = ""
    private let onSave: (ImportedRoute, BikeType) -> Void
    private let onClose: () -> Void

    public init(onSave: @escaping (ImportedRoute, BikeType) -> Void,
                onClose: @escaping () -> Void, sample: Bool = false) {
        _model = State(initialValue: PlannerPreviewModel(sample: sample))
        self.onSave = onSave
        self.onClose = onClose
    }

    public var body: some View {
        GeometryReader { geometry in
            PlannerPreviewMap(coordinates: model.geometry, pins: pins,
                              selectedID: mapPlaceShown || editorShown ? editingPointID ?? selectedPlace?.id : nil,
                              cursor: cursor, bottomInset: sheetHeight + geometry.safeAreaInsets.bottom,
                              fitRevision: fitRevision, onSelect: selectPin, onMapPoint: selectMapPoint,
                              showCycling: network == .cycling, showHiking: network == .hiking,
                              onVisibleMapRect: { visibleMapRect = $0 },
                              onSelectionPosition: { mapAnchor = $0 },
                              onVisibleRouteRange: { visibleRouteRange = $0 })
                .ignoresSafeArea(edges: .bottom)
                .overlay(alignment: .topTrailing) { mapTools.padding(12) }
                .overlay(alignment: .topLeading) {
                    if attributionShown && !layersShown {
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
                                                 highlighted: $highlightedCategories, counts: categoryCounts) {
                            layersShown = false; returnToPlanning()
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
                            Text("Route networks are a sample-area extract, available under the Open Database License.")
                                .font(.footnote).foregroundStyle(OBCTheme.secondary)
                        }
                        .padding(20).frame(width: min(330, geometry.size.width - 24))
                        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
                        .shadow(color: .black.opacity(0.14), radius: 14, y: 4).padding(12)
                    }
                }
                .overlay(alignment: .topLeading) {
                    if mapPlaceShown { mapContext(in: geometry.size) }
                }
                .sheet(isPresented: $drawerShown) {
                    PlannerPreviewDrawer(position: $drawerPosition,
                                         openHeight: min(panel == .planning ? planningHeight : listHeight, geometry.size.height - 80),
                                         expandedHeight: panel == .planning && model.hasRoute
                                            ? min(max(planningHeight + 120, geometry.size.height * 0.52), geometry.size.height - 48) : nil,
                                         onHeight: { sheetHeight = $0 }, header: { routeHeader }, content: { drawerContent })
                        .fullScreenCover(isPresented: $searchShown, onDismiss: finishSearch) {
                            PlannerPreviewSearch(model: model, initialQuery: searchQuery, initialRequest: queryRequest,
                                                 initialEditor: queryEditor, onResult: receiveSearch,
                                                 onCancel: cancelSearch,
                                                 onPlace: { searchSelectedPlace = $0; searchShown = false },
                                                 onQueryChange: { searchQuery = $0 },
                                                 onRequestChange: { queryRequest = $0 },
                                                 isInMapView: isInMapView)
                        }
                        .sheet(isPresented: $editorShown, onDismiss: finishOpeningSearch) { focusedEditor }
                        .alert("Save ride", isPresented: $saveShown) {
                            TextField("Ride name", text: $routeName)
                            Button("Cancel", role: .cancel) {}
                            Button("Save to Library") {
                                drawerShown = false
                                onSave(model.exportRoute(name: routeName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                                         ? model.routeTitle : routeName), model.bike)
                            }
                        } message: { Text(model.dayCount == 1
                            ? "Save this sample route in the mock library. It lasts until the app restarts."
                            : "This preview saves one route with the overnight waypoint. The day split is not saved. It lasts until the app restarts.") }
                        .confirmationDialog("Leave this preview?", isPresented: $closeShown, titleVisibility: .visible) {
                            Button("Discard and return to Library", role: .destructive) { drawerShown = false; onClose() }
                            Button("Cancel", role: .cancel) {}
                        }
                }
        }
        .navigationTitle("Plan a ride")
        .navigationBarTitleDisplayMode(.inline)
        .navigationBarBackButtonHidden()
        .toolbar(.visible, for: .navigationBar)
        .toolbarBackground(OBCTheme.page, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button { closeShown = true } label: { Label("Library", systemImage: "chevron.left") }
            }
            ToolbarItemGroup(placement: .topBarTrailing) {
                Button { model.undo() } label: { Image(systemName: "arrow.uturn.backward") }
                    .disabled(!model.canUndo).accessibilityLabel("Undo route change")
                    .contextMenu {
                        Button("Redo", systemImage: "arrow.uturn.forward") { model.redo() }.disabled(!model.canRedo)
                    }
                Button("Save", action: beginSave)
                    .fontWeight(.semibold).disabled(!model.hasRoute).accessibilityIdentifier("planner.save")
            }
        }
        .tint(OBCTheme.tint)
        .task { drawerShown = true }
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
            Button { fitRevision += 1; fraction = nil; mapPlaceShown = false } label: {
                Image(systemName: "arrow.up.left.and.arrow.down.right")
            }.accessibilityLabel("Show whole route")
            Button {
                mapPlaceShown = false; infoShown = false
                layersShown.toggle()
                if layersShown { drawerPosition = .collapsed }
            } label: { Image(systemName: "square.3.layers.3d") }
                .accessibilityLabel("Map layers")
            if network != .none {
                Button { layersShown = false; mapPlaceShown = false; infoShown.toggle() } label: {
                    Image(systemName: "info.circle")
                }.accessibilityLabel("Map data attribution")
            }
        }
        .font(.body.weight(.semibold)).buttonStyle(PlannerMapButtonStyle())
    }

    private var routeHeader: some View {
        HStack(spacing: 8) {
            if drawerPosition != .collapsed && panel != .planning {
                Text(panel == .results ? results?.title ?? "Places" : "Route points")
                    .font(.headline).lineLimit(1)
                Spacer(minLength: 4)
                doneButton(action: returnToPlanning)
            } else {
                Button { drawerPosition = .open } label: {
                    VStack(alignment: .leading, spacing: 3) {
                        if drawerPosition != .collapsed || !model.hasRoute {
                            Text(model.hasRoute ? model.routeTitle : (model.start == nil ? "Where shall we ride?" : "Choose a destination"))
                                .font(.headline).lineLimit(1)
                        }
                        if model.hasRoute { statsRow(model.stats) }
                    }.frame(maxWidth: .infinity, minHeight: 44, alignment: .leading).contentShape(Rectangle())
                }.buttonStyle(.plain).accessibilityHint("Show planning controls").accessibilityIdentifier("planner.routeSummary")
                if drawerPosition == .collapsed {
                    Button(action: startSearch) { Image(systemName: "magnifyingglass").frame(width: 44, height: 44) }
                        .accessibilityLabel("Search places").accessibilityIdentifier("planner.quickSearch")
                }
            }
        }.foregroundStyle(OBCTheme.ink)
    }

    @ViewBuilder private var drawerContent: some View {
        switch panel {
        case .planning:
            ScrollView {
                VStack(spacing: 10) {
                    if model.hasRoute { elevation(height: 56 + (drawerPosition == .expanded ? max(0, sheetHeight - planningHeight) : 0)) }
                    searchButton
                    HStack(spacing: 0) {
                        quickAction("Points", symbol: "point.topleft.down.to.point.bottomright.curvepath", target: .stops)
                        Spacer(minLength: 2)
                        quickAction(model.bike.name, symbol: "bicycle", target: .preferences)
                        Spacer(minLength: 2)
                        quickAction(model.dayCount == 1 ? "1 day" : "2 days", symbol: "sun.max", target: .days)
                    }
                }.padding(.horizontal, 16).padding(.bottom, 8)
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
            }
        case .stops:
            PlannerPreviewPoints(model: model, onEdit: editPoint, onAdd: startSearch,
                onReverse: { queryRequest = nil; results = model.lookup("reverse"); panel = .results },
                onExample: { model.loadSample(); resetPanel() },
                onNew: { model.newRoute(); resetPanel() })
        default: EmptyView()
        }
    }

    private func doneButton(action: @escaping () -> Void) -> some View {
        Button("Done", action: action)
            .font(.subheadline.weight(.semibold)).foregroundStyle(OBCTheme.onAmber)
            .padding(.horizontal, 18).frame(minHeight: 44).background(OBCTheme.amber, in: Capsule()).fixedSize(horizontal: true, vertical: false)
    }

    private func mapContext(in size: CGSize) -> some View {
        let width = min(310.0, size.width - 24)
        let x = min(max(12, mapAnchor.x - width / 2), size.width - width - 12)
        let available = max(240, size.height - sheetHeight - 28)
        let height = min(mapContextHeight, available)
        let preferredY = mapAnchor.y > height + 32 ? mapAnchor.y - height - 22 : mapAnchor.y + 22
        let y = min(max(12, preferredY), max(12, available - height))
        return ScrollView {
            VStack(spacing: 0) {
                HStack(spacing: 10) {
                    Image(systemName: selectedPlace?.kind.symbol ?? "mappin")
                        .font(.subheadline).foregroundStyle(OBCTheme.secondary)
                        .frame(width: 32, height: 32).background(OBCTheme.surface2, in: Circle())
                    Text(selectedPlace?.name ?? "Map point")
                        .font(.headline).frame(maxWidth: .infinity, alignment: .leading)
                    Button(action: dismissMapContext) {
                        Image(systemName: "xmark").font(.caption.weight(.semibold))
                            .foregroundStyle(OBCTheme.secondary)
                            .frame(width: 30, height: 30).background(OBCTheme.surface2, in: Circle())
                            .frame(width: 44, height: 44)
                    }.accessibilityLabel("Close point actions")
                }.padding(.leading, 16).padding(.trailing, 6).padding(.vertical, 6)
                Divider().overlay(OBCTheme.hairline)
                VStack(alignment: .leading, spacing: 4) {
                    if editingPointID != nil { pointEditor }
                    else if let selectedPlace { placeActions(selectedPlace) }
                }.padding(.horizontal, 16).padding(.vertical, 8)
            }
            .onGeometryChange(for: CGFloat.self, of: { $0.size.height }) { mapContextHeight = $0 }
        }
        .scrollBounceBehavior(.basedOnSize)
        .frame(width: width, height: height)
        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
        .shadow(color: .black.opacity(0.14), radius: 14, y: 4)
        .foregroundStyle(OBCTheme.ink).offset(x: x, y: y)
    }

    private func elevation(height: CGFloat) -> some View {
        PlannerPreviewProfile(routePoints: model.routePoints, height: height,
                              visibleRange: visibleRouteRange, selectedFraction: $fraction)
    }

    private var searchButton: some View {
        Button(action: startSearch) {
            HStack(spacing: 10) {
                Image(systemName: "magnifyingglass")
                Text(model.hasRoute ? "Find stops or change this ride" : "Search places or describe a ride")
                    .font(.subheadline).frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.horizontal, 14).frame(minHeight: 48)
            .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.controlRadius))
            .overlay(RoundedRectangle(cornerRadius: OBCTheme.controlRadius).stroke(OBCTheme.hairlineStrong, lineWidth: 1))
        }.buttonStyle(.plain).accessibilityIdentifier("planner.search")
    }

    private var focusedEditor: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    switch editorPanel {
                    case .preferences: preferences
                    case .days: days
                    case .point: pointEditor
                    default: EmptyView()
                    }
                }.padding(20).frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(OBCTheme.surface2).foregroundStyle(OBCTheme.ink)
            .navigationTitle(editorPanel == .preferences ? "Ride settings" : editorPanel == .days ? "Days" : "Point")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { editorShown = false; intent = .general }
                        .buttonStyle(.borderedProminent).buttonBorderShape(.capsule)
                        .tint(OBCTheme.amber).foregroundStyle(OBCTheme.onAmber)
                }
            }
        }
        .tint(OBCTheme.tint)
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    @ViewBuilder private var pointEditor: some View {
        if let point = (model.points + model.markers).first(where: { $0.id == editingPointID }) {
            if !mapPlaceShown { Text(point.place.name).font(.headline) }
            Picker("Point type", selection: Binding(get: { point.kind }, set: { model.setPointKind(id: point.id, kind: $0) })) {
                ForEach(PlannerPreviewPointKind.allCases, id: \.self) { Text($0.title).tag($0) }
            }.pickerStyle(.segmented)
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
            Text("How do you want to ride?").font(.title3.weight(.semibold))
            Picker("Bike", selection: Binding(get: { model.bike }, set: { model.setBike($0) })) {
                ForEach(BikeType.allCases, id: \.self) { Text($0.name).tag($0) }
            }.pickerStyle(.segmented)
            ForEach(PlannerPreviewPreset.allCases, id: \.self) { preset in
                Button { model.setPreset(preset) } label: {
                    HStack {
                        Text(preset.title)
                        Spacer()
                        if model.preset == preset { Image(systemName: "checkmark").fontWeight(.semibold) }
                    }.frame(minHeight: 44).contentShape(Rectangle())
                }.buttonStyle(.plain)
                Divider()
            }
            Text("In this preview, bike choice updates riding time. Routing preferences are selectable, but use the same sample route.")
                .font(.footnote).foregroundStyle(OBCTheme.secondary)
        }
    }

    private var days: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(model.dayCount == 1 ? "One good day out" : "Your two-day ride").font(.title3.weight(.semibold))
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
                Button("Change overnight stop", systemImage: "tent") { searchQuery = "camping"; openSearchFromDetail() }
                    .frame(minHeight: 44)
                Button("Make it a single day") { model.setOvernight(nil) }.buttonStyle(.obcGhost)
            } else {
                Text("Keep the whole route together, or choose a place to end your first day.")
                    .foregroundStyle(OBCTheme.secondary)
                Button("Add an overnight stop", systemImage: "tent") {
                    searchQuery = "camping"; queryRequest = PlannerPreviewPlaceQuery.parse("camping", hasRoute: model.hasRoute)
                    queryEditor = nil; intent = .general
                    results = queryRequest?.result(in: model, isInMapView: isInMapView) ?? model.lookup("camping")
                    editorShown = false; panel = .results; drawerPosition = .open
                }.buttonStyle(.obcPrimary)
                previewNote
            }
        }
    }

    @ViewBuilder private var resultContent: some View {
        if let results {
            if let action = results.action {
                Text(model.actionSummary(action))
                Text("Review this change. You can undo it after applying.")
                    .font(.subheadline).foregroundStyle(OBCTheme.secondary)
                Button(action.title) { model.apply(action); resetPanel() }.buttonStyle(.obcPrimary)
            } else if results.places.isEmpty {
                Text(results.explanation).foregroundStyle(OBCTheme.secondary)
                Button("Try another search") { resumeSearch() }.buttonStyle(.obcGhost)
            } else {
                ForEach(results.places) { place in
                    Button {
                        selectResult(place)
                    } label: {
                        HStack {
                            placeLabel(place)
                            Image(systemName: "chevron.right").font(.caption)
                        }.contentShape(Rectangle())
                    }.buttonStyle(.plain)
                    Divider()
                }
                Text(results.explanation).font(.footnote).foregroundStyle(OBCTheme.secondary)
            }
        }
    }

    private func placeLabel(_ place: PlannerPreviewPlace) -> some View {
        HStack(alignment: .center, spacing: 14) {
            Image(systemName: place.kind.symbol).font(.title3).foregroundStyle(OBCTheme.secondary).frame(width: 30)
            VStack(alignment: .leading, spacing: 4) {
                Text(place.name).font(.headline)
                if place.kind != .town, model.hasRoute {
                    Text("At \(place.alongRouteMeters / 1_000, specifier: "%.1f") km · \(Int(place.offRouteMeters)) m from route")
                        .font(.subheadline).foregroundStyle(OBCTheme.secondary)
                } else {
                    Text(place.kind.title).font(.subheadline).foregroundStyle(OBCTheme.secondary)
                }
            }
            Spacer(minLength: 0)
        }.frame(minHeight: 48).contentShape(Rectangle())
    }

    @ViewBuilder private func placeActions(_ place: PlannerPreviewPlace) -> some View {
        if case .replace(let id) = intent {
            Button("Use this place") { model.replacePoint(id: id, with: place); resetPanel() }.buttonStyle(.obcPrimary)
        } else if let existing = (model.points + model.markers).first(where: { $0.place.id == place.id }) {
            Button("Edit existing point") { editPoint(existing) }.buttonStyle(.obcPrimary)
        } else if model.start == nil {
            Button("Start here") { model.setStart(place); resetPanel() }.buttonStyle(.obcPrimary)
        } else if model.finish == nil {
            Button("Finish here") { model.setFinish(place); resetPanel() }.buttonStyle(.obcPrimary)
        } else {
            ForEach(PlannerPreviewPointKind.allCases, id: \.self) { kind in
                Button("Add \(kind.title.lowercased())", systemImage: kind.symbol) {
                    model.addPoint(place, kind: kind); resetPanel()
                }.frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            }
            if place.kind == .camping {
                Button("End day 1 here", systemImage: "moon") {
                    model.setOvernight(place); resetPanel()
                }.frame(minHeight: 44)
            }
        }
    }

    private func editPoint(_ point: PlannerPreviewPoint) {
        editingPointID = point.id; selectedPlace = point.place; show(.point)
    }

    private func quickAction(_ text: String, symbol: String, target: Panel) -> some View {
        Button { show(target) } label: { Label(text, systemImage: symbol).font(.subheadline.weight(.medium)).frame(minHeight: 44) }
            .buttonStyle(.plain).foregroundStyle(OBCTheme.secondary)
    }

    private func statsRow(_ stats: PlannerPreviewStats) -> some View {
        let distance = Text("\(stats.distanceMeters / 1_000, specifier: "%.1f") km")
        let ascent = Label("\(Int(stats.ascentMeters)) m", systemImage: "arrow.up.right")
        let duration = Text("\(Int(stats.seconds) / 3600)h \(Int(stats.seconds) / 60 % 60)m")
        return ViewThatFits(in: .horizontal) {
            HStack(spacing: 14) { distance; ascent; duration }.fixedSize(horizontal: true, vertical: false)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 14) { distance; duration }
                ascent
            }
        }.font(.subheadline.monospacedDigit()).foregroundStyle(OBCTheme.secondary)
    }

    private var previewNote: some View {
        Text("Interaction preview · sample route and places. Edited connections are illustrative.")
            .font(.footnote).foregroundStyle(OBCTheme.secondary)
    }

    private func startSearch() {
        searchSnapshot = nil
        mapPlaceShown = false; layersShown = false; infoShown = false
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
        queryRequest = nil; queryEditor = nil; mapPlaceShown = false
        if editorShown { searchAfterDismissal = true; editorShown = false }
        else { searchShown = true }
    }

    private func finishOpeningSearch() {
        guard searchAfterDismissal else { intent = .general; return }
        searchAfterDismissal = false; searchShown = true
    }

    private func finishSearch() {
        queryEditor = nil; searchSnapshot = nil
        if let place = searchSelectedPlace {
            searchSelectedPlace = nil; selectResult(place)
        }
    }

    private func beginSave() {
        guard model.hasRoute else { return }
        routeName = model.routeTitle; saveShown = true
    }

    private func show(_ panel: Panel) {
        mapPlaceShown = false; layersShown = false; infoShown = false
        if panel == .stops || panel == .results { self.panel = panel; drawerPosition = .open }
        else { editorPanel = panel; editorShown = true }
    }

    private func returnToPlanning() {
        panel = .planning; drawerPosition = .open; results = nil; fraction = nil; intent = .general
    }

    private func resetPanel() {
        mapPlaceShown = false; editorShown = false; selectedPlace = nil
        intent = .general; editingPointID = nil; queryRequest = nil
        returnToPlanning(); fitRevision += 1
    }

    private func receiveSearch(_ result: PlannerPreviewQueryResult) {
        results = result; selectedPlace = nil; editingPointID = nil
        panel = .results; drawerPosition = .open; searchShown = false
        if !result.places.isEmpty { fitRevision += 1 }
    }

    private func selectResult(_ place: PlannerPreviewPlace) {
        let selectionIntent = intent
        returnToPlanning(); intent = selectionIntent
        selectedPlace = place; editingPointID = nil; mapPlaceShown = true
    }

    private func selectPin(_ id: String, at location: CGPoint) {
        guard !hasMapContext else { dismissMapContext(); return }
        let selectionIntent = panel == .results ? intent : .general
        mapAnchor = location; layersShown = false; infoShown = false
        if let point = (model.points + model.markers).first(where: { $0.id == id }) {
            editingPointID = point.id; selectedPlace = point.place
        } else {
            guard let place = ((results?.places ?? []) + PlannerPreviewModel.sampleMapPlaces).first(where: { $0.id == id }) else { return }
            editingPointID = nil; selectedPlace = place
        }
        panel = .planning; results = nil; intent = selectionIntent; mapPlaceShown = true
    }

    private func selectMapPoint(_ coordinate: Coordinate, at location: CGPoint) {
        guard !hasMapContext else { dismissMapContext(); return }
        let selectionIntent = panel == .results ? intent : .general
        mapAnchor = location; editingPointID = nil; layersShown = false; infoShown = false
        selectedPlace = .init(id: UUID().uuidString, name: "Map point", coordinate: coordinate)
        panel = .planning; results = nil; intent = selectionIntent; mapPlaceShown = true
    }

    private var hasMapContext: Bool { mapPlaceShown || layersShown || infoShown }

    private func dismissMapContext() {
        mapPlaceShown = false; layersShown = false; infoShown = false
        selectedPlace = nil; editingPointID = nil; intent = .general
    }

    private var categoryCounts: [PlannerPreviewPlaceCategory: Int] {
        Dictionary(PlannerPreviewPlaceCategory.allCases.map { category in
            (category, PlannerPreviewModel.sampleMapPlaces.filter { PlannerPreviewPlaceCategory.category(for: $0) == category }.count)
        }, uniquingKeysWith: +)
    }

    private func isInMapView(_ place: PlannerPreviewPlace) -> Bool {
        visibleMapRect.contains(MKMapPoint(CLLocationCoordinate2D(latitude: place.coordinate.latitude, longitude: place.coordinate.longitude)))
    }

    private var pins: [PlannerPreviewMapPin] {
        var pins = model.points.enumerated().map { index, point in
            PlannerPreviewMapPin(id: point.id, title: point.place.name, coordinate: point.place.coordinate,
                symbol: point.id == model.overnightPointID ? "moon.fill" : point.place.kind.symbol,
                kind: index == 0 ? .start : index == model.points.count - 1 ? .finish : point.kind == .shape ? .shape : .place)
        }
        pins += model.markers.map { .init(id: $0.id, title: $0.place.name, coordinate: $0.place.coordinate, symbol: "mappin", kind: .marker) }
        var places = results?.places ?? []
        places += PlannerPreviewModel.sampleMapPlaces.filter { place in
            guard let category = PlannerPreviewPlaceCategory.category(for: place) else { return model.points.isEmpty }
            return results == nil && !hiddenCategories.contains(category)
        }
        if mapPlaceShown, let selectedPlace, editingPointID == nil { places.insert(selectedPlace, at: 0) }
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
        let line = MeasuredLine(routePoints: model.geometry.map { RoutePoint(coordinate: $0) })
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
