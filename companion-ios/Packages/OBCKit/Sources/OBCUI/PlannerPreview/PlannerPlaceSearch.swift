import Foundation
import Observation
import OBCPlanner

@MainActor @Observable
final class PlannerPlaceSearch {
    struct Query: Equatable {
        var text = ""
        var request: PlannerPreviewPlaceQuery?
    }

    private(set) var draft = Query()
    private(set) var accepted = Query()
    private(set) var results: PlannerPreviewQueryResult?
    private(set) var isSearching = false
    private(set) var searchError: String?
    private(set) var selectedPlace: PlannerPreviewPlace?
    private(set) var detailsError: String?
    private var remote: PlannerPreviewQueryResult?
    @ObservationIgnored private let model: PlannerPreviewModel
    @ObservationIgnored private let debounce: @Sendable () async throws -> Void
    @ObservationIgnored private var viewBounds: [Double]?
    @ObservationIgnored private var isInMapView: (PlannerPreviewPlace) -> Bool = { _ in true }
    @ObservationIgnored private var searchRevision = 0
    @ObservationIgnored private var selectionRevision = 0
    @ObservationIgnored private(set) var searchTask: Task<Void, Never>?
    @ObservationIgnored private(set) var detailsTask: Task<Void, Never>?

    init(model: PlannerPreviewModel,
         debounce: @escaping @Sendable () async throws -> Void = { try await Task.sleep(for: .milliseconds(250)) }) {
        self.model = model
        self.debounce = debounce
    }

    deinit { searchTask?.cancel(); detailsTask?.cancel() }

    var hasQuery: Bool { !draft.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    var result: PlannerPreviewQueryResult { remote ?? model.lookup(draft.text) }
    var canSubmit: Bool { hasQuery && !isSearching && searchError == nil }

    /// A nil query edits the accepted search. New text starts a separate draft.
    func begin(query: String? = nil, viewBounds: [Double]?, isInMapView: @escaping (PlannerPreviewPlace) -> Bool) {
        self.viewBounds = viewBounds
        self.isInMapView = isInMapView
        draft = query.map { Query(text: $0, request: interpret($0)) } ?? accepted
        refresh()
    }

    func setQuery(_ text: String) {
        draft = Query(text: text, request: interpret(text))
        refresh()
    }

    func setRequest(_ request: PlannerPreviewPlaceQuery) {
        draft.request = request
        refresh()
    }

    func accept() -> PlannerPreviewQueryResult? {
        guard canSubmit else { return nil }
        let result = result
        accepted = draft; results = result
        model.mapPlaces = result.places
        stopSearch()
        return result
    }

    func cancel() {
        stopSearch()
        draft = accepted; remote = nil; searchError = nil
    }

    func clearResults() { results = nil; accepted = Query() }

    func showAction(_ text: String) {
        cancel()
        accepted = Query(text: text)
        results = model.lookup(text)
    }

    private func interpret(_ text: String) -> PlannerPreviewPlaceQuery? {
        guard model.lookup(text).action == nil else { return nil }
        return PlannerPreviewPlaceQuery.parse(text, hasRoute: model.hasRoute)
    }

    private func stopSearch() {
        searchRevision += 1
        searchTask?.cancel(); searchTask = nil
        isSearching = false
    }

    private func refresh() {
        stopSearch()
        remote = nil; searchError = nil
        guard hasQuery, model.lookup(draft.text).action == nil else { return }
        isSearching = true
        let revision = searchRevision, request = draft.request
        let query = request?.serverQuery(text: draft.text, view: viewBounds, model: model)
            ?? PlannerSearchQuery(text: draft.text, view: viewBounds)
        let model = model, debounce = debounce, isInMapView = isInMapView, length = model.routeLine.length
        searchTask = Task { [weak self] in
            do {
                try await debounce()
                try Task.checkCancellation()
                let found = try await model.searchPlaces(query)
                try Task.checkCancellation()
                guard let self, revision == self.searchRevision else { return }
                let places = request?.filter(found, routeLengthMeters: length, isInMapView: isInMapView, matchesName: false) ?? found
                self.remote = .init(title: request.map { $0.kinds.isEmpty ? $0.name : $0.kindLabel } ?? "Places",
                                    explanation: "", places: places, action: nil)
                self.isSearching = false
            } catch {
                guard let self, revision == self.searchRevision else { return }
                self.isSearching = false
                if !(error is CancellationError) { self.searchError = error.localizedDescription }
            }
        }
    }

    func select(_ place: PlannerPreviewPlace?) {
        selectionRevision += 1
        detailsTask?.cancel(); detailsTask = nil
        selectedPlace = place; detailsError = nil
        guard let place, !place.detailsLoaded,
              place.id.range(of: #"^(?:[nwr]|Q)[1-9][0-9]*$"#, options: .regularExpression) != nil else { return }
        let coordinate = place.coordinate, revision = selectionRevision, model = model
        var query = PlannerSearchQuery(text: "Place", view: [coordinate.longitude - 0.01, coordinate.latitude - 0.01,
                                                           coordinate.longitude + 0.01, coordinate.latitude + 0.01])
        query.source = place.id
        detailsTask = Task { [weak self] in
            do {
                let details = try await model.searchPlaces(query).first
                try Task.checkCancellation()
                guard let self, revision == self.selectionRevision, let details else { return }
                self.selectedPlace = .init(id: place.id, name: place.name, coordinate: coordinate, kind: place.kind,
                    alongRouteMeters: place.alongRouteMeters, offRouteMeters: place.offRouteMeters,
                    hours: details.hours, note: details.note, website: details.website, phone: details.phone,
                    description: details.description, content: details.content, detailsLoaded: true)
            } catch {
                guard let self, revision == self.selectionRevision, !Task.isCancelled else { return }
                self.detailsError = "Place details are unavailable. Try selecting the place again."
            }
        }
    }
}
