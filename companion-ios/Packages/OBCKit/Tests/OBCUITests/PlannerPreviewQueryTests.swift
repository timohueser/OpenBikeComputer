#if DEBUG
import Testing
@testable import OBCUI

@MainActor
struct PlannerPreviewQueryTests {
    @Test func compoundRequestFiltersCategoryRouteSectionAndRadius() throws {
        let query = try #require(PlannerPreviewPlaceQuery.parse("cafés and water between 10 and 20 km within 100 m", hasRoute: true))
        #expect(query.kinds == [.cafe, .water] && query.area == .section)
        #expect(query.fromMeters == 10_000 && query.toMeters == 20_000 && query.radiusMeters == 100)
        let places = query.filter(PlannerPreviewModel.sampleMapPlaces, routeLengthMeters: 30_524, isInMapView: { _ in false })
        #expect(places.map(\.id) == ["cafe", "water"])
    }

    @Test func editingFiltersChangesResultsWithoutChangingRoute() throws {
        let model = PlannerPreviewModel(sample: true)
        var query = try #require(PlannerPreviewPlaceQuery.parse("cafés", hasRoute: true))
        let places = PlannerPreviewModel.sampleMapPlaces
        #expect(query.filter(places, routeLengthMeters: 30_524, isInMapView: { _ in true }).count == 3)
        query.radiusMeters = 100
        #expect(query.filter(places, routeLengthMeters: 30_524, isInMapView: { _ in true }).count == 2)
        query.kinds = [.water]
        #expect(query.filter(places, routeLengthMeters: 30_524, isInMapView: { _ in true }).allSatisfy { $0.kind == .water })
        _ = query.result(in: model, isInMapView: { _ in true })
        #expect(!model.canUndo && model.points.count == 2)
    }

    @Test func mapScopeUsesVisiblePlacesAndUnrecognizedTextStaysANameSearch() throws {
        let query = try #require(PlannerPreviewPlaceQuery.parse("cafés in this map view", hasRoute: true))
        #expect(query.area == .view)
        let places = query.filter(PlannerPreviewModel.sampleMapPlaces, routeLengthMeters: 30_524, isInMapView: { $0.id == "cafe-orchard" })
        #expect(places.map(\.id) == ["cafe-orchard"])
        let unknown = try #require(PlannerPreviewPlaceQuery.parse("An unknown village", hasRoute: false))
        #expect(unknown.kinds.isEmpty && unknown.name == "An unknown village")
        #expect(unknown.filter(PlannerPreviewModel.sampleMapPlaces, routeLengthMeters: 0, isInMapView: { _ in true }).isEmpty)
    }
}
#endif
