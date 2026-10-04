#if DEBUG
import Testing
import Foundation
import OBCDomain
import OBCPlanner
@testable import OBCUI

@MainActor
struct PlannerPreviewModelTests {
    @Test func previewAndApplyAreSeparateAndNewRouteIsOneUndoStep() async throws {
        let model = PlannerPreviewModel(service: PlannerTestSource())
        let query = model.lookup("Freiburg to Titisee")
        #expect(!model.hasRoute && !model.canUndo)
        model.apply(try #require(query.action))
        #expect(!model.canSave && model.geometry.isEmpty)
        await model.calculateRoute()
        #expect(model.hasRoute && model.geometry.count > 1 && model.canSave)
        model.addPoint(PlannerPreviewModel.sampleMapPlaces[2])
        model.mapPlaces = PlannerPreviewModel.sampleMapPlaces
        model.apply(.splitDays)
        await model.calculateRoute()
        let geometry = model.geometry
        let orderedPoints = model.points
        let export = model.exportRoute(name: "Black Forest")
        #expect(export.name == "Black Forest")
        #expect(export.creator == "OpenBikeComputer")
        #expect(export.points.map(\.coordinate) == geometry && export.waypoints.count == 2)
        model.newRoute()
        #expect(!model.hasRoute && model.geometry.isEmpty && model.points.isEmpty && model.overnight == nil)
        model.undo()
        await model.calculateRoute()
        #expect(model.geometry == geometry && model.points == orderedPoints && model.dayCount == 2)
        model.redo()
        #expect(!model.hasRoute)
        model.setStart(PlannerPreviewModel.sampleMapPlaces[0])
        #expect(!model.canRedo)
    }

    @Test func daySplitSumsTotalsAndReverseKeepsPointOrder() async {
        let model = PlannerPreviewModel(sample: true, service: PlannerTestSource())
        model.addPoint(PlannerPreviewModel.sampleMapPlaces[2])
        model.addPoint(PlannerPreviewModel.sampleMapPlaces[3])
        model.mapPlaces = PlannerPreviewModel.sampleMapPlaces
        model.apply(.splitDays)
        await model.calculateRoute()
        let distance = model.stats.distanceMeters
        #expect(model.dayStats.count == 2)
        #expect(abs(model.dayStats.reduce(0) { $0 + $1.distanceMeters } - distance) < 0.01)
        #expect(model.dayStats.allSatisfy { $0.distanceMeters > 0 })
        let points = model.points
        model.apply(.reverse)
        await model.calculateRoute()
        #expect(model.start?.id == "titisee" && model.finish?.id == "freiburg")
        #expect(model.points == Array(points.reversed()))
        #expect(model.canSave && model.stats.distanceMeters > 0)
        model.undo()
        #expect(model.start?.id == "freiburg" && model.dayCount == 2 && model.points == points)
        model.setOvernight(nil)
        #expect(model.dayCount == 1)
    }

    @Test func unsupportedQueriesAndDuplicateVisitsDoNotChangeThePlan() {
        let model = PlannerPreviewModel(sample: true, service: PlannerTestSource())
        let unsupported = model.lookup("avoid every hill and find a hotel with a pool")
        #expect(unsupported.action == nil && unsupported.places.isEmpty)
        #expect(!model.canUndo)
        let cafe = PlannerPreviewModel.sampleMapPlaces[2]
        model.addPoint(cafe)
        model.addPoint(cafe)
        #expect(model.points.count == 3)
        model.undo()
        #expect(model.points.count == 2 && !model.canUndo)
    }

    @Test func endpointsFollowRouteOrderAndReplacementKeepsPointIdentity() throws {
        let model = PlannerPreviewModel(sample: true, service: PlannerTestSource())
        let cafe = PlannerPreviewModel.sampleMapPlaces[2]
        model.addPoint(cafe)
        model.movePoint(fromOffsets: IndexSet(integer: 1), toOffset: 0)
        #expect(model.start == cafe && model.points[1].place.id == "freiburg")
        model.undo()
        #expect(model.start?.id == "freiburg" && model.points[1].place == cafe)
        model.removePoint(id: "freiburg")
        #expect(model.start == cafe && model.finish?.id == "titisee")
        let startID = try #require(model.points.first?.id)
        model.replacePoint(id: startID, with: PlannerPreviewModel.sampleMapPlaces[3])
        #expect(model.points.first?.id == startID && model.start?.id == "water")
        model.replacePoint(id: startID, with: PlannerPreviewModel.sampleMapPlaces[1])
        #expect(model.start?.id == "water")
        model.removePoint(id: "titisee")
        #expect(model.start?.id == "water" && model.finish == nil && !model.hasRoute)
        model.addPoint(cafe)
        #expect(Set(model.points.map(\.id)).count == model.points.count)
    }

    @Test func markersLeaveTheLineAloneAndShapesDoNotExportStops() async {
        let model = PlannerPreviewModel(sample: true, service: PlannerTestSource())
        let cafe = PlannerPreviewModel.sampleMapPlaces[2]
        await model.calculateRoute()
        let initial = model.routePoints
        model.addPoint(cafe, kind: .marker)
        #expect(model.routePoints == initial && model.markers.count == 1)
        model.setPointKind(id: cafe.id, kind: .shape)
        await model.calculateRoute()
        let shaped = model.routePoints
        #expect(shaped != initial && model.markers.isEmpty && model.points.count == 3)
        #expect(model.exportRoute(name: "Ride").waypoints.isEmpty)
        model.setPointKind(id: cafe.id, kind: .visit)
        #expect(model.routePoints == shaped && model.exportRoute(name: "Ride").waypoints.count == 1)
        #expect(model.routePoints.map(\.coordinate) == model.geometry)
    }

    @Test func overnightMovesWithItsPointAndEndpointMovesClearItAtomically() async throws {
        let model = PlannerPreviewModel(sample: true, service: PlannerTestSource())
        model.addPoint(PlannerPreviewModel.sampleMapPlaces[2])
        model.addPoint(PlannerPreviewModel.sampleMapPlaces[3])
        model.mapPlaces = PlannerPreviewModel.sampleMapPlaces
        model.apply(.splitDays)
        await model.calculateRoute()
        let id = try #require(model.overnightPointID)
        model.movePoint(fromOffsets: IndexSet(integer: 3), toOffset: 1)
        await model.calculateRoute()
        #expect(model.overnightPointID == id && model.points[1].id == id && model.dayCount == 2)
        #expect(abs(model.dayStats[0].distanceMeters - (model.pointDistances[id] ?? -1)) < 0.01)
        model.movePoint(fromOffsets: IndexSet(integer: 1), toOffset: model.points.count)
        #expect(model.points.last?.id == id && model.overnightPointID == nil && model.dayCount == 1)
        model.undo()
        #expect(model.points[1].id == id && model.overnightPointID == id && model.dayCount == 2)
        model.movePoint(fromOffsets: IndexSet(integer: 1), toOffset: 0)
        #expect(model.points.first?.id == id && model.overnightPointID == nil)
        model.undo()
        #expect(model.overnightPointID == id)
    }
    @Test func mapSelectedPlacesUseTheCurrentRoutePosition() async {
        let model = PlannerPreviewModel(sample: true, service: PlannerTestSource())
        await model.calculateRoute()
        let middle = model.routeLine.coordinate(at: model.routeLine.length / 2)
        let place = PlannerPreviewPlace(id: "tile-poi", name: "Water", coordinate:
            Coordinate(latitude: middle.latitude + 0.001, longitude: middle.longitude), kind: .water, hours: "24/7")
        let positioned = model.positionedPlace(place)
        #expect(positioned.alongRouteMeters > 0 && positioned.alongRouteMeters < model.routeLine.length)
        #expect(positioned.offRouteMeters > 0)
        #expect(positioned.id == place.id && positioned.hours == place.hours && positioned.kind == .water)
    }
    @Test func staleRepliesAndFailuresCannotReplaceOrSaveTheCurrentRoute() async {
        let source = ControlledPlannerSource()
        let model = PlannerPreviewModel(sample: true, service: source)
        let first = Task { await model.calculateRoute() }
        await source.waitForRequest(1)
        model.setBike(.road)
        let second = Task { await model.calculateRoute() }
        await source.waitForRequest(2)
        await source.finish(1)
        await second.value
        let accepted = model.routePoints
        #expect(model.canSave && model.stats.seconds == 1)
        await source.finish(0)
        await first.value
        #expect(model.routePoints == accepted && model.stats.seconds == 1)
        model.setPreset(.shorter)
        #expect(!model.canSave && model.geometry.isEmpty)
        let failed = Task { await model.calculateRoute() }
        await source.waitForRequest(3)
        await source.finish(2, fail: true)
        await failed.value
        #expect(!model.canSave && model.geometry.isEmpty && model.routeError != nil)
    }

    @Test func backToStartRoutesTheWholeRoundAndUndoOpensThePlan() async {
        let model = PlannerPreviewModel(sample: true, service: PlannerTestSource())
        model.addPoint(PlannerPreviewModel.sampleMapPlaces[2])
        await model.calculateRoute()
        let open = model.stats.distanceMeters, points = model.points
        model.closeLoop()
        await model.calculateRoute()
        #expect(model.isLoop && model.points == points && model.finish == model.start)
        #expect(model.routeTitle == "Loop from Freiburg" && model.stats.distanceMeters > open)
        #expect(model.routePoints.last?.coordinate == model.start?.coordinate && model.pointDistances.count == 3)
        #expect(model.exportRoute(name: "").waypoints.map(\.name) == ["Valley café", "Titisee"])
        model.undo()
        #expect(!model.isLoop && model.finish?.id == "titisee")
    }

    @Test func aLoopKeepsItsStartThroughEdits() {
        let model = PlannerPreviewModel(sample: true, service: PlannerTestSource())
        let cafe = PlannerPreviewModel.sampleMapPlaces[2], water = PlannerPreviewModel.sampleMapPlaces[3]
        model.closeLoop()
        model.addPoint(cafe)
        #expect(model.points.map(\.id) == ["freiburg", "titisee", "cafe"])
        model.movePoint(fromOffsets: IndexSet(integer: 0), toOffset: 2)
        model.movePoint(fromOffsets: IndexSet(integer: 2), toOffset: 0)
        model.movePoint(fromOffsets: IndexSet(integer: 2), toOffset: 1)
        #expect(model.points.map(\.id) == ["freiburg", "cafe", "titisee"])
        model.apply(.reverse)
        #expect(model.isLoop && model.points.map(\.id) == ["freiburg", "titisee", "cafe"])
        model.setFinish(water)
        #expect(!model.isLoop && model.finish == water && model.points.count == 4)
        model.undo()
        model.removePoint(id: "titisee")
        #expect(model.isLoop && model.hasRoute)
        model.removePoint(id: "cafe")
        #expect(!model.isLoop && !model.hasRoute)
    }

    @Test func movingTheStartKeepsTheOrderAroundTheLoop() async {
        let model = PlannerPreviewModel(sample: true, service: PlannerTestSource())
        model.addPoint(PlannerPreviewModel.sampleMapPlaces[2])
        model.closeLoop()
        model.startLoop(at: "titisee")
        #expect(model.points.map(\.id) == ["titisee", "freiburg", "cafe"] && model.points[1].kind == .visit)
        model.setOvernightPoint(id: "cafe")
        #expect(!model.canMoveLoopStart)
        model.startLoop(at: "cafe")
        #expect(model.start?.id == "titisee")
        model.setOvernightPoint(id: nil)

        // A closed square with one shaping point. The route turns back at its start.
        let route = try! JSONDecoder().decode(CatalogRecord.self, from: Data("""
        {"id":1,"kind":"hiking","name":"Square","rank":1,"loop":true,"length_m":4000,"ascent_m":0,"descent_m":0,"cells":[],
         "line_udeg":[8000000,47900000,10000,0,0,10000,-10000,0,0,-10000],"via":[2],"turnarounds":[0]}
        """.utf8))
        let line = route.line
        model.makeLoop(route)
        await model.calculateRoute()
        #expect(model.isLoop && model.routeTitle == "Loop from Start" && model.points.map(\.kind) == [.visit, .shape])
        #expect(model.routePoints.map(\.coordinate) == [line[0], line[2], line[0]])
        #expect(model.points.map(\.turnaround) == [true, false] && model.exportRoute(name: "").waypoints.isEmpty)
        model.startLoop(at: model.points[1].id)
        #expect(model.points.map(\.place.coordinate) == [line[2], line[0]] && model.points.map(\.turnaround) == [false, true])
        model.undo(); model.undo()
        #expect(model.points.map(\.id) == ["titisee", "freiburg", "cafe"])
    }
}

private actor PlannerTestSource: PlannerDataSource {
    func release() async throws -> PlannerRelease { testRelease }
    func route(points: [Coordinate], bike: BikeType, preference: RoutePreference, release: PlannerRelease) async throws -> PlannedPath {
        let samples = points.map { RoutePoint(coordinate: $0, elevationMeters: 300) }
        let length = MeasuredLine(routePoints: samples).length
        let elapsed = MeasuredLine(routePoints: samples).vertices.map { $0.distance / 4 }
        return PlannedPath(points: samples, distance: length, ascent: 0, seconds: length / 4,
                           pointIndices: Array(points.indices), elapsed: elapsed)
    }
    func search(_ query: PlannerSearchQuery, release: PlannerRelease) async throws -> [PlannerPlace] { [] }
    func overlays(bounds: [Double], zoom: Double, network: String, release: PlannerRelease) async throws -> Data { Data() }
}

private actor ControlledPlannerSource: PlannerDataSource {
    private var requests: [[Coordinate]] = []
    private var replies: [Int: CheckedContinuation<PlannedPath, any Error>] = [:]
    private var waiters: [(Int, CheckedContinuation<Void, Never>)] = []
    func release() async throws -> PlannerRelease { testRelease }
    func route(points: [Coordinate], bike: BikeType, preference: RoutePreference, release: PlannerRelease) async throws -> PlannedPath {
        try await withCheckedThrowingContinuation { continuation in
            let index = requests.count
            requests.append(points); replies[index] = continuation
            for waiter in waiters where requests.count >= waiter.0 { waiter.1.resume() }
            waiters.removeAll { requests.count >= $0.0 }
        }
    }
    func waitForRequest(_ count: Int) async {
        if requests.count >= count { return }
        await withCheckedContinuation { waiters.append((count, $0)) }
    }
    func finish(_ index: Int, fail: Bool = false) {
        guard let reply = replies.removeValue(forKey: index) else { return }
        if fail { reply.resume(throwing: PlannerFailure.noRoad); return }
        let points = requests[index].map { RoutePoint(coordinate: $0) }
        reply.resume(returning: PlannedPath(points: points, distance: 1000, ascent: 0, seconds: Double(index),
                                           pointIndices: Array(points.indices), elapsed: [0,Double(index)]))
    }
    func search(_ query: PlannerSearchQuery, release: PlannerRelease) async throws -> [PlannerPlace] { [] }
    func overlays(bounds: [Double], zoom: Double, network: String, release: PlannerRelease) async throws -> Data { Data() }
}

private let testRelease: PlannerRelease = try! JSONDecoder().decode(PlannerRelease.self, from: Data("""
{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","region":"test","bounds":[7,47,9,49],"basemap":"https://test/basemap.json","glyphs":"https://test/fonts/{fontstack}/{range}.pbf","sprites":"https://test/sprites","terrain":"https://test/{z}/{x}/{y}.webp","terrain_attribution":"Terrain","search":"https://test/search","routing":"https://test/routing","manifest":"https://test/manifest.json"}
""".utf8))
#endif
