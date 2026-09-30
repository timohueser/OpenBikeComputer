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
    @State private var edited: Bool
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
        _edited = State(initialValue: initialRequest != nil)
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
                                PlannerPreviewQuerySummary(request: request, edited: edited) { field in
                                    isFocused = false
                                    activeEditor = activeEditor == field ? nil : field
                                }
                                if let activeEditor {
                                    PlannerPreviewQueryEditor(field: activeEditor, request: request, hasRoute: model.hasRoute,
                                                              routeLengthMeters: model.stats.distanceMeters) { next in
                                        self.request = next; edited = true; self.activeEditor = nil
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
                        if hasQuery { results }
                        if !hasQuery || result.places.isEmpty {
                            VStack(alignment: .leading, spacing: 4) {
                                Text("Try an example").font(.headline).padding(.bottom, 4)
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
            request = Self.interpret(value, model: model); edited = false; activeEditor = nil
            onQueryChange(value); onRequestChange(request)
        }
        .task { onRequestChange(request); isFocused = !hasQuery && activeEditor == nil }
    }

    private var searchField: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").foregroundStyle(OBCTheme.secondary)
            TextField("Find a place, or ask along your route…", text: $query)
                .font(.body).foregroundStyle(OBCTheme.ink).focused($isFocused)
                .submitLabel(.search).autocorrectionDisabled().onSubmit { submit() }
                .accessibilityLabel("Find a place or change the route")
                .accessibilityIdentifier("planner.searchField")
            if !query.isEmpty {
                Button { query = ""; isFocused = true } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(OBCTheme.secondary).frame(width: 44, height: 44)
                }
                .accessibilityLabel("Clear search")
            }
        }
        .frame(minHeight: 48).padding(.leading, 12).padding(.trailing, query.isEmpty ? 12 : 0)
        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusSmall).stroke(OBCTheme.hairlineStrong, lineWidth: 1))
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
                        .fontWeight(.semibold).frame(minHeight: 44).buttonStyle(.borderedProminent)
                        .tint(OBCTheme.amber).foregroundStyle(OBCTheme.onAmber)
                        .accessibilityIdentifier("planner.searchReview")
                }
            } else {
                Button("Show \(result.places.count) \(result.places.count == 1 ? "place" : "places") on map", systemImage: "map") { submit() }
                    .font(.subheadline.weight(.semibold)).frame(minHeight: 44)
                    .buttonStyle(.borderedProminent).tint(OBCTheme.secondary).foregroundStyle(OBCTheme.surface)
                    .buttonBorderShape(.capsule).accessibilityIdentifier("planner.searchShowPlaces")
                ForEach(result.places) { place in
                    Button { select(place) } label: { placeRow(place) }
                        .buttonStyle(.plain).accessibilityIdentifier("planner.searchPlace.\(place.id)")
                    Divider().overlay(OBCTheme.hairline)
                }
                Text(result.explanation).font(.caption).foregroundStyle(OBCTheme.secondary)
            }
        }
    }

    private func placeRow(_ place: PlannerPreviewPlace) -> some View {
        HStack(spacing: 12) {
            Image(systemName: place.kind.symbol).frame(width: 24).foregroundStyle(OBCTheme.secondary)
            VStack(alignment: .leading, spacing: 4) {
                Text(place.name).font(.body).foregroundStyle(OBCTheme.ink)
                if model.hasRoute && place.kind != .town {
                    Text("At \(place.alongRouteMeters / 1_000, specifier: "%.1f") km · \(Int(place.offRouteMeters)) m off route")
                        .font(.caption).foregroundStyle(OBCTheme.secondary)
                } else {
                    Text(place.kind.title).font(.caption).foregroundStyle(OBCTheme.secondary)
                }
            }
            Spacer(minLength: 8)
            Image(systemName: "chevron.right").font(.caption).foregroundStyle(OBCTheme.secondary)
        }
        .frame(minHeight: 52).contentShape(Rectangle())
    }

    private static func interpret(_ query: String, model: PlannerPreviewModel) -> PlannerPreviewPlaceQuery? {
        guard model.lookup(query).action == nil else { return nil }
        return PlannerPreviewPlaceQuery.parse(query, hasRoute: model.hasRoute)
    }
    private func submit() { guard hasQuery else { return }; isFocused = false; onResult(result) }
    private func select(_ place: PlannerPreviewPlace) {
        isFocused = false
        if let onPlace { onPlace(place) }
        else { onResult(.init(title: place.name, explanation: result.explanation, places: [place], action: nil)) }
    }
}
#endif
