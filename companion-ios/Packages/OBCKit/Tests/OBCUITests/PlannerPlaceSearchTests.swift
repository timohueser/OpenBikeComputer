import Foundation
import OBCDomain
import OBCPlanner
import Testing
@testable import OBCUI

@MainActor
struct PlannerPlaceSearchTests {
    @Test func staleSuccessAndFailureCannotReplaceTheLatestSearch() async throws {
        let source = SearchSource(), search = makeSearch(source)
        search.begin(query: "cafe", viewBounds: nil, isInMapView: { _ in true })
        let first = try #require(search.searchTask)
        await source.waitForRequest(1)
        search.setQuery("water")
        let second = try #require(search.searchTask)
        await source.waitForRequest(2)
        search.setQuery("cafe")
        let latest = try #require(search.searchTask)
        await source.waitForRequest(3)

        await source.finish(0, failure: true)
        await first.value
        #expect(search.isSearching && search.searchError == nil && !search.canSubmit)
        await source.finish(2, places: [place("n3", kind: "cafe")])
        await latest.value
        await source.finish(1, places: [place("n2", kind: "water")])
        await second.value
        #expect(search.result.places.map(\.id) == ["n3"])
        #expect(!search.isSearching && search.searchError == nil && search.canSubmit)

        search.setQuery("shop")
        let failing = try #require(search.searchTask)
        await source.waitForRequest(4)
        await source.finish(3, failure: true)
        await failing.value
        #expect(search.searchError != nil && !search.isSearching && !search.canSubmit)
        search.setQuery("Freiburg to Titisee")
        #expect(search.result.action == .createSample && search.searchError == nil && search.canSubmit)
        #expect(search.searchTask == nil)
    }

    @Test func cancelRestoresAcceptedTextFiltersAndResults() async throws {
        let source = SearchSource(), search = makeSearch(source)
        search.begin(query: "cafe", viewBounds: [7, 47, 9, 49], isInMapView: { $0.id != "n2" })
        await source.waitForRequest(1)
        await source.finish(0, places: [place("n1", kind: "cafe"), place("n2", kind: "cafe")])
        await search.searchTask?.value
        #expect(search.accept()?.places.map(\.id) == ["n1"])
        let accepted = search.accepted

        search.begin(viewBounds: nil, isInMapView: { _ in true })
        let resumed = try #require(search.searchTask)
        await source.waitForRequest(2)
        search.setQuery("water")
        let edited = try #require(search.searchTask)
        await source.waitForRequest(3)
        var request = try #require(search.draft.request)
        request.name = "Village"
        search.setRequest(request)
        let filtered = try #require(search.searchTask)
        await source.waitForRequest(4)
        search.cancel()
        await source.finish(1, places: [])
        await source.finish(2, failure: true)
        await source.finish(3, places: [place("n4", kind: "water")])
        await resumed.value; await edited.value; await filtered.value
        #expect(search.draft == accepted && search.accepted == accepted)
        #expect(search.results?.places.map(\.id) == ["n1"])
        #expect(!search.isSearching && search.searchError == nil)

        search.begin(viewBounds: nil, isInMapView: { _ in true })
        await source.waitForRequest(5)
        await source.finish(4, places: [place("n5", kind: "cafe")])
        await search.searchTask?.value
        #expect(search.draft == accepted && search.accept()?.places.map(\.id) == ["n5"])
    }

    @Test func selectedDetailsKeepThePickedPlaceAndIgnoreSupersededReplies() async throws {
        let source = SearchSource(), search = makeSearch(source)
        let selected = PlannerPreviewPlace(id: "n1", name: "Map café", coordinate: .init(latitude: 48, longitude: 8),
                                          kind: .cafe, alongRouteMeters: 200, offRouteMeters: 10)
        search.select(selected)
        let first = try #require(search.detailsTask)
        await source.waitForRequest(1)
        search.select(selected)
        let retry = try #require(search.detailsTask)
        await source.waitForRequest(2)
        await source.finish(0, failure: true)
        await first.value
        #expect(search.selectedPlace == selected && search.detailsError == nil)
        await source.finish(1, places: [place("n1", kind: "cafe")])
        await retry.value
        let detailed = try #require(search.selectedPlace)
        #expect(detailed.id == selected.id && detailed.name == selected.name && detailed.coordinate == selected.coordinate)
        #expect(detailed.kind == selected.kind && detailed.alongRouteMeters == 200 && detailed.offRouteMeters == 10)
        #expect(detailed.detailsLoaded && detailed.website == "https://cafe.test" && detailed.hours == "24/7")

        search.select(selected)
        let failing = try #require(search.detailsTask)
        await source.waitForRequest(3)
        await source.finish(2, failure: true)
        await failing.value
        #expect(search.detailsError != nil && search.selectedPlace == selected)
        search.select(selected)
        let dismissed = try #require(search.detailsTask)
        await source.waitForRequest(4)
        #expect(search.detailsError == nil)
        search.select(nil)
        await source.finish(3, places: [place("n1", kind: "cafe")])
        await dismissed.value
        #expect(search.selectedPlace == nil && search.detailsError == nil)
        #expect(await source.requests.allSatisfy { $0.source == "n1" })
    }

    private func makeSearch(_ source: SearchSource) -> PlannerPlaceSearch {
        PlannerPlaceSearch(model: PlannerPreviewModel(service: source), debounce: {})
    }

    private func place(_ id: String, kind: String) -> PlannerPlace {
        try! JSONDecoder().decode(PlannerPlace.self, from: Data("""
        {"source":"\(id)","name":"Place","city":"Village","kind":"\(kind)","lon":8.1,"lat":48.1,
         "opening_hours":"24/7","website":"https://cafe.test"}
        """.utf8))
    }
}

private actor SearchSource: PlannerDataSource {
    private(set) var requests: [PlannerSearchQuery] = []
    private var replies: [Int: CheckedContinuation<[PlannerPlace], any Error>] = [:]
    private var waiters: [(Int, CheckedContinuation<Void, Never>)] = []
    func release() async throws -> PlannerRelease {
        let host = URL(string: "https://planner.test")!
        return PlannerRelease(id: String(repeating: "a", count: 64), region: "test", bounds: [7, 47, 9, 49], basemap: host,
                              places: host, glyphs: "", sprites: "", terrain: "", terrain_attribution: "", search: host,
                              routing: host, manifest: host, overlays: host)
    }
    func route(points: [Coordinate], turnarounds: [Int], activity: RouteActivity,
               preference: RoutePreference, release: PlannerRelease) async throws -> PlannedPath { throw PlannerFailure.unavailable }
    func search(_ query: PlannerSearchQuery, release: PlannerRelease) async throws -> [PlannerPlace] {
        try await withCheckedThrowingContinuation { continuation in
            replies[requests.count] = continuation
            requests.append(query)
            for waiter in waiters where requests.count >= waiter.0 { waiter.1.resume() }
            waiters.removeAll { requests.count >= $0.0 }
        }
    }
    func waitForRequest(_ count: Int) async {
        if requests.count >= count { return }
        await withCheckedContinuation { waiters.append((count, $0)) }
    }
    func finish(_ index: Int, places: [PlannerPlace] = [], failure: Bool = false) {
        guard let reply = replies.removeValue(forKey: index) else { return }
        if failure { reply.resume(throwing: PlannerFailure.unavailable) }
        else { reply.resume(returning: places) }
    }
}
