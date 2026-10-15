import Foundation
import OBCDomain
import OBCPlanner
import Testing
@testable import OBCUI

@MainActor
struct PlannerRouteOptionsTests {
    private func planned(_ source: OptionsSource) async -> (PlannerPreviewModel, PlannerRouteOptions) {
        let model = PlannerPreviewModel(service: source)
        model.setStart(.init(id: "freiburg", name: "Freiburg", coordinate: start))
        model.setFinish(.init(id: "hinterzarten", name: "Hinterzarten", coordinate: finish))
        await model.calculateRoute()
        let options = PlannerRouteOptions()
        await options.load(model)
        return (model, options)
    }

    @Test func aPreviewLeavesThePlanAndUseIsOneUndoStep() async throws {
        let source = OptionsSource(), (model, options) = await planned(source)
        // Your route is the plan's own, not the primary route of the options answer.
        let yours = try #require(options.yours(model))
        #expect(yours.option.path.distance == 30_700 && options.items.map(\.name) == ["Shorter", "Via Kirchzarten"])
        let shorter = try #require(options.items.first { $0.id == "B" })
        #expect(PlannerRouteOptions.difference(shorter.option.path, from: yours.option.path, locale: english) == "5.5 km shorter · 237 m less climbing")
        #expect(PlannerRouteOptions.accessibilityLabel(shorter, yours: yours, locale: english)
                == "B, Shorter, 25.2 kilometres, 669 metres climbing, 5.5 km shorter, 237 m less climbing")

        let plan = model.exportPlan(), depth = model.undoDepth, revision = model.routingRevision
        options.select("B", model: model)
        #expect(options.preview?.name == "Shorter" && options.letterPlaces != nil)
        #expect(model.exportPlan() == plan && model.undoDepth == depth && model.routingRevision == revision && model.canSave)
        options.cancel()
        #expect(options.preview == nil && model.exportPlan() == plan)

        options.select("B", model: model)
        options.use(model)
        #expect(model.preset == .shorter && model.points.count == 2 && model.undoDepth == depth + 1 && options.preview == nil)
        // The set stays: the route that Use replaced is listed under its own name.
        #expect(options.isCurrent(model) && options.items.map(\.name) == ["Via Kirchzarten", "Balanced"])
        await model.calculateRoute()
        options.select("C", model: model)
        options.use(model)
        #expect(model.preset == .balanced && model.undoDepth == depth + 2 && options.items.map(\.name) == ["Via Kirchzarten", "Shorter"])
    }

    @Test func theCorridorBecomesAShapingPointAndStaleOptionsAreRefused() async throws {
        let source = OptionsSource(), (model, options) = await planned(source)
        options.select("C", model: model)
        model.setActivity(.road)
        await model.calculateRoute()
        let depth = model.undoDepth
        options.use(model)
        #expect(model.points.count == 2 && model.undoDepth == depth && model.preset == .balanced)

        await options.load(model)
        options.select("C", model: model)
        options.use(model)
        #expect(model.points.map(\.kind) == [.visit, .shape, .visit] && model.points[1].place.coordinate == via)
        #expect(model.undoDepth == depth + 1)
        // Another route of the same set takes the corridor's via away again.
        await model.calculateRoute()
        options.select("B", model: model)
        options.use(model)
        #expect(model.points.count == 2 && model.preset == .shorter && model.undoDepth == depth + 2)

        model.setLegMode(try #require(model.leg(into: model.points[1].id)), to: .straight)
        #expect(!model.offersRouteOptions)
    }

    enum Naming: CaseIterable { case hamletOnly, failure, slow }
    @Test(arguments: Naming.allCases)
    func theCorridorIsTheOtherWayWithoutASettlementInTime(_ naming: Naming) async {
        let source = OptionsSource()
        await source.name(naming == .hamletOnly ? .hamlet : naming == .failure ? .failure : .slow)
        let model = PlannerPreviewModel(service: source)
        model.setStart(.init(id: "freiburg", name: "Freiburg", coordinate: start))
        model.setFinish(.init(id: "hinterzarten", name: "Hinterzarten", coordinate: finish))
        let options = PlannerRouteOptions()
        options.nameTimeout = .milliseconds(50)
        await options.load(model)
        #expect(options.items.last?.name == "Other way")
    }
}

private let english = Locale(identifier: "en_GB")
private let start = Coordinate(latitude: 47.995, longitude: 7.85)
private let finish = Coordinate(latitude: 47.903, longitude: 8.106)
private let via = Coordinate(latitude: 47.962, longitude: 7.95)

/// Your route is 30.7 km with 906 m of climbing. Shorter is 25.2 km with 669 m.
private actor OptionsSource: PlannerDataSource {
    enum Naming { case kirchzarten, hamlet, failure, slow }
    private var naming = Naming.kirchzarten
    func name(_ naming: Naming) { self.naming = naming }

    func release() async throws -> PlannerRelease { optionsRelease }
    func route(points: [Coordinate], turnarounds: [Int], activity: RouteActivity, preference: RoutePreference,
               release: PlannerRelease) async throws -> PlannedPath {
        path(points, meters: preference == .balanced ? 30_700 : 25_200, climb: preference == .balanced ? 906 : 669)
    }
    func options(points: [Coordinate], activity: RouteActivity, preference: RoutePreference,
                 release: PlannerRelease) async throws -> [RouteOption] {
        let mine = try await route(points: points, turnarounds: [], activity: activity, preference: preference, release: release)
        let other: RoutePreference = preference == .balanced ? .shorter : .lessClimbing
        return [RouteOption(reason: .primary, preference: preference,
                            path: path(points, meters: mine.distance + 300, climb: mine.ascent)),
                RouteOption(reason: preference == .balanced ? .shorter : .lessClimbing, preference: other,
                            path: path([points[0], Coordinate(latitude: 47.97, longitude: 8.0), points[1]], meters: 25_200, climb: 669)),
                RouteOption(reason: .corridor, preference: preference, via: via,
                            path: path([points[0], via, points[1]], meters: 30_100, climb: 886))]
    }
    func search(_ query: PlannerSearchQuery, release: PlannerRelease) async throws -> [PlannerPlace] {
        switch naming {
        case .failure: throw PlannerFailure.unavailable
        case .slow: try await Task.sleep(for: .seconds(5))
        case .kirchzarten, .hamlet: break
        }
        let places = [#"{"source":"n1","name":"Dietenbach","city":"Kirchzarten","kind":"hamlet","lon":7.9426,"lat":47.9545}"#]
            + (naming == .kirchzarten ? [#"{"source":"n2","name":"Kirchzarten","city":"","kind":"city","lon":7.9573,"lat":47.9651}"#] : [])
        return try places.map { try JSONDecoder().decode(PlannerPlace.self, from: Data($0.utf8)) }
    }

    private func path(_ coordinates: [Coordinate], meters: Double, climb: Double) -> PlannedPath {
        PlannedPath(points: coordinates.map { RoutePoint(coordinate: $0, elevationMeters: 300) }, distance: meters, ascent: climb,
                    seconds: meters / 4, pointIndices: [0, coordinates.count - 1], elapsed: coordinates.indices.map(Double.init))
    }
}

private let optionsRelease = PlannerRelease(
    id: String(repeating: "a", count: 64), region: "test", bounds: [7, 47, 9, 49], basemap: URL(string: "https://test")!,
    places: URL(string: "https://test")!, glyphs: "", sprites: "", terrain: "", terrain_attribution: "",
    search: URL(string: "https://test")!, routing: URL(string: "https://test")!, manifest: URL(string: "https://test")!,
    overlays: URL(string: "https://test")!)
