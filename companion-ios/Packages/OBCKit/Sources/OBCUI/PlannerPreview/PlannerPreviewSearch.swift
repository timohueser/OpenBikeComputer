#if DEBUG && os(iOS)
import SwiftUI

struct PlannerPreviewSearch: View {
    let model: PlannerPreviewModel
    let onResult: (PlannerPreviewQueryResult) -> Void
    let onCancel: () -> Void
    let onPlace: ((PlannerPreviewPlace) -> Void)?
    let onQueryChange: (String) -> Void
    let onRequestChange: (PlannerPreviewPlaceQuery?) -> Void
    let isInMapView: (PlannerPreviewPlace) -> Bool
    @State private var query: String
    @State private var request: PlannerPreviewPlaceQuery?
    @State private var activeEditor: PlannerPreviewQueryField?
    @FocusState private var isFocused: Bool

    init(model: PlannerPreviewModel, initialQuery: String = "", initialRequest: PlannerPreviewPlaceQuery? = nil,
         initialEditor: PlannerPreviewQueryField? = nil,
         onResult: @escaping (PlannerPreviewQueryResult) -> Void, onCancel: @escaping () -> Void,
         onPlace: ((PlannerPreviewPlace) -> Void)? = nil,
         onQueryChange: @escaping (String) -> Void = { _ in },
         onRequestChange: @escaping (PlannerPreviewPlaceQuery?) -> Void = { _ in },
         isInMapView: @escaping (PlannerPreviewPlace) -> Bool = { _ in true }) {
        self.model = model; self.onResult = onResult; self.onCancel = onCancel; self.onPlace = onPlace
        self.onQueryChange = onQueryChange; self.onRequestChange = onRequestChange; self.isInMapView = isInMapView
        _query = State(initialValue: initialQuery)
        _request = State(initialValue: initialRequest ?? Self.interpret(initialQuery, model: model))
        _activeEditor = State(initialValue: initialEditor)
    }

    /// One prompt for the drawer's search button and the field it opens.
    static func prompt(hasRoute: Bool) -> String {
        hasRoute ? "Find stops or change this route" : "Search places or describe a route"
    }

    private var hasQuery: Bool { !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    private var result: PlannerPreviewQueryResult { request?.result(in: model, isInMapView: isInMapView) ?? model.lookup(query) }
    private var suggestions: [(title: String, symbol: String)] {
        model.hasRoute
            ? [("Cafés along route", "cup.and.saucer"), ("Water", "drop"), ("Shops", "basket"),
               ("Cafés between 10 and 20 km", "point.topleft.down.to.point.bottomright.curvepath"),
               ("Reverse route", "arrow.uturn.backward"), ("Two days", "moon")]
            : [("Freiburg", "mappin"), ("Titisee", "mappin")]
    }

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 0) {
                searchField.padding(.horizontal, 20).padding(.vertical, 12)
                ScrollView {
                    VStack(alignment: .leading, spacing: 20) {
                        if let request {
                            VStack(alignment: .leading, spacing: 10) {
                                PlannerPreviewQuerySummary(request: request) { field in
                                    isFocused = false
                                    activeEditor = activeEditor == field ? nil : field
                                }
                                if let activeEditor {
                                    PlannerPreviewQueryEditor(field: activeEditor, request: request, hasRoute: model.hasRoute,
                                                              routeLengthMeters: model.stats.distanceMeters) { next in
                                        self.request = next; self.activeEditor = nil
                                        onRequestChange(next)
                                    } onCancel: { self.activeEditor = nil }
                                    .id(activeEditor)
                                } else if request.area != .view && request.radiusMeters == nil {
                                    Button("Add radius filter", systemImage: "plus") {
                                        isFocused = false; activeEditor = .radius
                                    }
                                    .font(.subheadline).frame(minHeight: 44)
                                }
                            }
                        }
                        // A filter editor is the one open task; the results return once it closes.
                        if hasQuery, activeEditor == nil { results }
                        if !hasQuery || result.places.isEmpty {
                            VStack(alignment: .leading, spacing: 4) {
                                Text("Examples").font(.headline).padding(.bottom, 4)
                                ForEach(suggestions, id: \.title) { suggestion in
                                    Button { query = suggestion.title; isFocused = false } label: {
                                        HStack(spacing: 12) {
                                            Image(systemName: suggestion.symbol).frame(width: 24)
                                            Text(suggestion.title)
                                            Spacer(minLength: 8)
                                            Image(systemName: "arrow.up.left").font(.caption)
                                        }
                                        .foregroundStyle(OBCTheme.ink).frame(minHeight: 44).contentShape(Rectangle())
                                    }
                                    .buttonStyle(.plain)
                                }
                            }
                        }
                    }
                    .padding(.horizontal, 20).padding(.bottom, 24)
                }
                .scrollDismissesKeyboard(.interactively)
            }
            .background(OBCTheme.page)
            .navigationTitle("Search")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", action: onCancel).fixedSize()
                        .accessibilityLabel("Cancel search")
                        .accessibilityIdentifier("planner.searchCancel")
                }
            }
        }
        .tint(OBCTheme.tint)
        .onChange(of: query) { _, value in
            request = Self.interpret(value, model: model); activeEditor = nil
            onQueryChange(value); onRequestChange(request)
        }
        .task { onRequestChange(request); isFocused = !hasQuery && activeEditor == nil }
    }

    /// `OBCSearchField` with focus and submit, which the shared field does not carry.
    private var searchField: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").font(.subheadline.weight(.semibold)).foregroundStyle(OBCTheme.secondary)
            TextField(Self.prompt(hasRoute: model.hasRoute), text: $query)
                .font(.subheadline).foregroundStyle(OBCTheme.ink).focused($isFocused)
                .submitLabel(.search).autocorrectionDisabled().onSubmit { submit() }
                .accessibilityIdentifier("planner.searchField")
            if !query.isEmpty {
                Button { query = ""; isFocused = true } label: {
                    Image(systemName: "xmark.circle.fill").font(.subheadline).foregroundStyle(OBCTheme.secondary)
                        .frame(width: 44, height: 44).contentShape(Rectangle())
                }
                .accessibilityLabel("Clear search")
            }
        }
        .frame(minHeight: 44).padding(.leading, 12).padding(.trailing, query.isEmpty ? 12 : 0)
        .background(OBCTheme.fill, in: RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
    }

    private var results: some View {
        let result = result
        return VStack(alignment: .leading, spacing: 12) {
            if result.places.isEmpty {
                Text(result.action == nil && request != nil ? "No places match these filters" : result.title)
                    .font(.headline).foregroundStyle(OBCTheme.ink)
                Text(result.action == nil && request != nil ? "Try another place type, a wider radius, or a different search area." : result.explanation)
                    .font(.subheadline).foregroundStyle(OBCTheme.secondary)
                if result.action != nil {
                    Button("Review change") { submit() }
                        .buttonStyle(.obcPrimary).accessibilityIdentifier("planner.searchReview")
                }
            } else {
                Button("Show \(result.places.count) \(result.places.count == 1 ? "place" : "places") on map", systemImage: "map") { submit() }
                    .buttonStyle(.obcPrimary).accessibilityIdentifier("planner.searchShowPlaces")
                ForEach(result.places) { place in
                    Button { select(place) } label: { PlannerPlaceRow(place: place, showsRouteDistances: model.hasRoute) }
                        .buttonStyle(.plain).accessibilityIdentifier("planner.searchPlace.\(place.id)")
                    Divider().overlay(OBCTheme.hairline)
                }
                Text(result.explanation).font(.caption).foregroundStyle(OBCTheme.secondary)
            }
        }
    }

    private static func interpret(_ query: String, model: PlannerPreviewModel) -> PlannerPreviewPlaceQuery? {
        guard model.lookup(query).action == nil else { return nil }
        return PlannerPreviewPlaceQuery.parse(query, hasRoute: model.hasRoute)
    }
    private func submit() { guard hasQuery else { return }; isFocused = false; onResult(result) }
    // A picked place carries its list along, so the planner can offer the way back to it.
    private func select(_ place: PlannerPreviewPlace) {
        isFocused = false
        if let onPlace { onResult(result); onPlace(place) }
        else { onResult(.init(title: place.name, explanation: result.explanation, places: [place], action: nil)) }
    }
}

/// One place row for the search screen and the drawer's results: the kind's glyph, the name,
/// and where it sits relative to the route.
struct PlannerPlaceRow: View {
    let place: PlannerPreviewPlace
    let showsRouteDistances: Bool

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: place.kind.symbol).frame(width: 24).foregroundStyle(OBCTheme.secondary)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(place.name).font(.system(.body, weight: .semibold)).foregroundStyle(OBCTheme.ink)
                Text(detail).font(.system(.subheadline).monospacedDigit()).foregroundStyle(OBCTheme.secondary)
            }
            Spacer(minLength: 8)
            Image(systemName: "chevron.right").font(.system(.caption, weight: .semibold)).foregroundStyle(OBCTheme.secondary)
                .accessibilityHidden(true)
        }
        .frame(minHeight: 52).contentShape(Rectangle())
    }

    private var detail: String { Self.detail(for: place, showsRouteDistances: showsRouteDistances) }

    /// "Café · 13.9 km · 80 m off route", or the kind alone without a route. The map card drops
    /// the kind, which its glyph already shows, so the line fits on one row.
    static func detail(for place: PlannerPreviewPlace, showsRouteDistances: Bool, includesKind: Bool = true) -> String {
        guard showsRouteDistances, place.kind != .town else { return place.kind.title }
        let distances = "\(OBCFormat.distance(meters: place.alongRouteMeters)) · \(OBCFormat.shortDistance(meters: place.offRouteMeters)) off route"
        return includesKind ? "\(place.kind.title) · \(distances)" : distances
    }
}
#endif
