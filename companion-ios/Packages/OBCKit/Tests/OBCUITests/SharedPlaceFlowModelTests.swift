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

    /// The share extension encodes every character outside the unreserved set.
    @Test func theAppLinkCarriesTheSharedTextUnchanged() throws {
        let text = "Café Atlantik & Bar = 1+1\nhttps://maps.app.goo.gl/abc?g_st=ic"
        let unreserved = CharacterSet(charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")
        let encoded = try #require(text.addingPercentEncoding(withAllowedCharacters: unreserved))
        let link = try #require(URL(string: "openbikecomputer://place?text=\(encoded)"))
        #expect(SharedPlaceFlowModel.sharedText(in: link) == text)
        #expect(SharedPlaceFlowModel.sharedText(in: try #require(URL(string: "openbikecomputer://place"))) == nil)
        #expect(SharedPlaceFlowModel.sharedText(in: try #require(URL(string: "https://openbikecomputer.com/place?text=x"))) == nil)
    }
}

private actor SearchSource: PlannerDataSource {
    private let places: [PlannerPlace]
    init(places: [PlannerPlace] = []) { self.places = places }
    func release() async throws -> PlannerRelease { sharedPlaceRelease }
    func route(points: [Coordinate], turnarounds: [Int], activity: RouteActivity, preference: RoutePreference,
               release: PlannerRelease) async throws -> PlannedPath { throw PlannerFailure.unavailable }
    func search(_ query: PlannerSearchQuery, release: PlannerRelease) async throws -> [PlannerPlace] { places }
}

private let sharedPlaceRelease: PlannerRelease = {
    let host = URL(string: "https://test")!
    return PlannerRelease(id: String(repeating: "a", count: 64), region: "test", bounds: [7, 47, 9, 49], basemap: host, places: host,
                          glyphs: "", sprites: "", terrain: "", terrain_attribution: "", search: host, routing: host,
                          manifest: host, overlays: host)
}()
