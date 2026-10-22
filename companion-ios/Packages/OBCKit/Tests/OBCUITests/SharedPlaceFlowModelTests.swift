import Foundation
import Testing
import OBCDomain
import OBCPlanner
import OBCTransport
@testable import OBCUI

/// A place shared from a map app: the bike-type choice, the position and the place make one planner seed.
@MainActor @Suite struct SharedPlaceFlowModelTests {
    private let here = Coordinate(latitude: 47.99, longitude: 7.84)
    private let cafe = Coordinate(latitude: 48.0, longitude: 8.0)

    private func freshStore() throws -> LastBikeTypeStore {
        let suite = "obc.test.sharedPlace.\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defaults.removePersistentDomain(forName: suite)
        return LastBikeTypeStore(defaults: defaults)
    }

    private func flow(locate: Coordinate?, resolved: SharedPlace, store: LastBikeTypeStore) -> SharedPlaceFlowModel {
        SharedPlaceFlowModel(locate: { locate }, resolve: { _ in resolved }, lastBikeType: store)
    }

    @Test func theLastBikeTypeIsTheDefaultAndTheChoiceIsRemembered() async throws {
        let store = try freshStore()
        store.value = .gravel
        let model = flow(locate: here, resolved: .coordinate(cafe, name: "Café Atlantik"), store: store)
        model.receive("Café Atlantik", using: SearchSource())
        #expect(model.pending?.bikeType == .gravel)

        model.choose(.mtb)
        model.proceed()
        try await waitFor("the seed") { model.seed != nil }
        #expect(store.value == .mtb)
        #expect(model.seed == PlannerPreviewSeed(start: here, finish: cafe, finishName: "Café Atlantik", activity: .mtb))
        #expect(!model.isWaiting)
    }

    @Test func withoutAPositionThePlannerHasNoStart() async throws {
        let model = flow(locate: nil, resolved: .coordinate(cafe, name: nil), store: try freshStore())
        model.receive("Café Atlantik", using: SearchSource())
        model.choose(.road)
        model.proceed()
        try await waitFor("the seed") { model.seed != nil }
        #expect(model.seed?.start == nil)
        #expect(model.seed?.finish == cafe)
    }

    @Test func aPlaceOutsideTheRegionSaysSoInsteadOfOpeningThePlanner() async throws {
        let far = Coordinate(latitude: 10, longitude: 10)
        let model = flow(locate: here, resolved: .coordinate(far, name: "Faraway"), store: try freshStore())
        model.receive("Faraway", using: SearchSource())
        model.choose(.road)
        model.proceed()
        try await waitFor("the notice") { model.notice != nil }
        #expect(model.notice == "This place is outside the map region of the planner.")
        #expect(model.seed == nil)
    }

    @Test func aQueryResolvesThroughThePlannerSearch() async throws {
        let place = try JSONDecoder().decode(PlannerPlace.self, from: Data("""
        {"source":"osm:1","name":"Café Atlantik","city":"Freiburg","kind":"cafe","lon":7.85,"lat":47.99}
        """.utf8))
        let model = flow(locate: here, resolved: .query("Atlantik Freiburg"), store: try freshStore())
        model.receive("Atlantik Freiburg", using: SearchSource(places: [place]))
        model.choose(.touring)
        model.proceed()
        try await waitFor("the seed") { model.seed != nil }
        #expect(model.seed?.finish == place.coordinate)
        #expect(model.seed?.finishName == "Café Atlantik")
    }

    /// Online, the search covers the whole world; offline, the offline maps answer without a view.
    @Test func aQuerySearchesTheWorldFirstAndWithoutAViewWhenThatFails() async throws {
        let place = try JSONDecoder().decode(PlannerPlace.self, from: Data("""
        {"source":"osm:1","name":"Café Atlantik","city":"Freiburg","kind":"cafe","lon":7.85,"lat":47.99}
        """.utf8))
        let source = SearchSource(places: [place], offline: true)
        let model = flow(locate: here, resolved: .query("Atlantik Freiburg"), store: try freshStore())
        model.receive("Atlantik Freiburg", using: source)
        model.choose(.road)
        model.proceed()
        try await waitFor("the seed") { model.seed != nil }
        #expect(model.seed?.finish == place.coordinate)
        #expect(await source.views == [[-180, -90, 180, 90], nil])
    }

    @Test func aSheetClosedWithoutAChoiceDropsTheShareAndASecondShareReplacesTheFirst() async throws {
        let model = SharedPlaceFlowModel(locate: { nil }, resolve: { $0 }, lastBikeType: try freshStore())
        model.receive("https://maps.apple.com/?ll=47.99,7.84&q=Home", using: SearchSource())
        model.pending = nil
        model.proceed()
        #expect(model.pending == nil && !model.isWaiting)

        model.receive("https://maps.apple.com/?ll=47.99,7.84&q=Home", using: SearchSource())
        let link = try #require(URL(string: "openbikecomputer://place?text=https%3A%2F%2Fmaps.apple.com%2F%3Fll%3D48.0%2C8.0%26q%3DAtlantik"))
        model.receive(try #require(SharedPlaceFlowModel.sharedText(in: link)), using: SearchSource())
        #expect(model.pending?.name == "Atlantik")
        model.choose(.road)
        model.proceed()
        try await waitFor("the seed") { model.seed != nil }
        #expect(model.seed?.finish == cafe)
        #expect(model.seed?.finishName == "Atlantik")
    }
}

private actor SearchSource: PlannerDataSource {
    private let places: [PlannerPlace]
    /// Fails a search with a view, as the online search does without a connection.
    private let offline: Bool
    private(set) var views: [[Double]?] = []
    init(places: [PlannerPlace] = [], offline: Bool = false) { self.places = places; self.offline = offline }
    func release() async throws -> PlannerRelease { sharedPlaceRelease }
    func route(points: [Coordinate], turnarounds: [Int], activity: RouteActivity, preference: RoutePreference,
               release: PlannerRelease) async throws -> PlannedPath { throw PlannerFailure.unavailable }
    func search(_ query: PlannerSearchQuery, release: PlannerRelease) async throws -> [PlannerPlace] {
        views.append(query.view)
        if offline, query.view != nil { throw PlannerFailure.offlineUnavailable }
        return places
    }
}

private let sharedPlaceRelease: PlannerRelease = {
    let host = URL(string: "https://test")!
    return PlannerRelease(id: String(repeating: "a", count: 64), region: "test", bounds: [7, 47, 9, 49], basemap: host, places: host,
                          glyphs: "", sprites: "", terrain: "", terrain_attribution: "", search: host, routing: host,
                          manifest: host, overlays: host)
}()
