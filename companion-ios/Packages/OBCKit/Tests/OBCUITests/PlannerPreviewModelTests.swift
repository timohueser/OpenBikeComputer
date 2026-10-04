#if DEBUG
import Testing
import Foundation
import OBCDomain
import OBCPlanner
@testable import OBCUI

@MainActor
struct PlannerPreviewModelTests {
    @Test func projectionsAndMapFeaturesRetainPlaceDetails() async throws {
        let model = PlannerPreviewModel(sample: true, service: PlannerTestSource())
        await model.calculateRoute()
        let data = Data(#"{"source":"n123","name":"Camp","city":"","kind":"campsite","lon":8,"lat":48,"website":"camp.example","phone":"+49 123","description":"Small tents only."}"#.utf8)
        let record = try JSONDecoder().decode(PlannerPlace.self, from: data)
        let place = PlannerPreviewPlace(id: record.source, name: record.name, coordinate: record.coordinate, kind: .camping,
            website: record.website, phone: record.phone, description: record.description)
        let positioned = model.positionedPlace(place)
        #expect(positioned.website == place.website && positioned.phone == place.phone && positioned.description == place.description)
        let features = try #require(JSONSerialization.jsonObject(with: NativePlaceKind.geoJSON([record])) as? [String: Any])
        let properties = try #require((features["features"] as? [[String: Any]])?.first?["properties"] as? [String: Any])
        #expect(properties["website"] as? String == place.website && properties["description"] as? String == place.description)
        for (type, letter) in [(1,"n"),(2,"w"),(3,"r")] {
            #expect(NativePlaceKind.source(for: NSNumber(value: (Int64(type) << 44) | 123)) == "\(letter)123")
        }
        #expect(NativePlaceKind.source(for: "n123") == "n123")
    }
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
        #expect(!model.hasRoute && model.geometry.isEmpty && model.points.isEmpty && model.nights.isEmpty)
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
        model.clearNights()
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
        let id = try #require(model.nights.first?.id)
        model.movePoint(fromOffsets: IndexSet(integer: 3), toOffset: 1)
        await model.calculateRoute()
        #expect(model.nights.first?.id == id && model.points[1].id == id && model.dayCount == 2)
        #expect(abs(model.dayStats[0].distanceMeters - (model.pointDistances[id] ?? -1)) < 0.01)
        model.movePoint(fromOffsets: IndexSet(integer: 1), toOffset: model.points.count)
        #expect(model.points.last?.id == id && model.nights.first?.id == nil && model.dayCount == 1)
        model.undo()
        #expect(model.points[1].id == id && model.nights.first?.id == id && model.dayCount == 2)
        model.movePoint(fromOffsets: IndexSet(integer: 1), toOffset: 0)
        #expect(model.points.first?.id == id && model.nights.first?.id == nil)
        model.undo()
        #expect(model.nights.first?.id == id)
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
        model.setActivity(.road)
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

    @Test func aSignedRoutePlanReplacesThePlanInOneUndoStep() async throws {
        let source = PlannerTestSource()
        let model = PlannerPreviewModel(sample: true, service: source)
        model.setPreset(.shorter)
        let before = model.points
        let points = (0...3).map { Coordinate(latitude: 47.9, longitude: 8 + Double($0) / 100) }
        model.planSignedRoute(RoutePlan(points: points, turnarounds: [2]), loop: false, name: "Westweg · Stage 9",
                              startName: "Titisee", finishName: nil)
        await model.calculateRoute()
        #expect(model.routeTitle == "Westweg · Stage 9" && model.start?.name == "Titisee" && model.finish?.name == "Finish")
        #expect(model.points.map(\.kind) == [.visit, .shape, .shape, .visit] && model.preset == .balanced)
        let requested = await source.turnarounds
        #expect(model.routePoints.map(\.coordinate) == points && requested == [[2]])
        model.undo()
        #expect(model.points == before && model.preset == .shorter && model.routeTitle == "Freiburg → Titisee")
    }

    @Test func movingTheStartKeepsTheOrderAroundTheLoop() async {
        let source = PlannerTestSource()
        let model = PlannerPreviewModel(sample: true, service: source)
        model.addPoint(PlannerPreviewModel.sampleMapPlaces[2])
        model.closeLoop()
        model.startLoop(at: "titisee")
        #expect(model.points.map(\.id) == ["titisee", "freiburg", "cafe"] && model.points[1].kind == .visit)
        model.setNight(id: "cafe", true)
        #expect(!model.canMoveLoopStart)
        model.startLoop(at: "cafe")
        #expect(model.start?.id == "titisee")
        model.setNight(id: "cafe", false)

        // A closed square with one shaping point. The route turns back at its start.
        let route = try! JSONDecoder().decode(CatalogRecord.self, from: Data("""
        {"id":1,"kind":"hiking","name":"Square","rank":1,"loop":true,"length_m":4000,"ascent_m":0,"descent_m":0,"cells":[],
         "line_udeg":[8000000,47900000,10000,0,0,10000,-10000,0,0,-10000],"via":[2],"turnarounds":[0]}
        """.utf8))
        let line = route.line
        model.planSignedRoute(try! #require(route.plan), loop: true, name: "Square", startName: "Rieslehof", finishName: nil)
        await model.calculateRoute()
        #expect(model.isLoop && model.routeTitle == "Square" && model.start?.name == "Rieslehof")
        #expect(model.points.map(\.kind) == [.visit, .shape] && model.preset == .balanced)
        #expect(model.routePoints.map(\.coordinate) == [line[0], line[2], line[0]])
        #expect(model.points.map(\.turnaround) == [true, false] && model.exportRoute(name: "").waypoints.isEmpty)
        // The start turns back only once it is an interior point.
        model.startLoop(at: model.points[1].id)
        await model.calculateRoute()
        #expect(model.points.map(\.place.coordinate) == [line[2], line[0]] && model.points.map(\.turnaround) == [false, true])
        let requested = await source.turnarounds
        #expect(requested == [[], [1]])
        model.undo(); model.undo()
        #expect(model.points.map(\.id) == ["titisee", "freiburg", "cafe"] && model.planName == nil)
    }

    @Test func aDrawnLegSplitsWithoutChangeAndAReplacedPointRoutesOnlyItsTwoLegs() async throws {
        let points = [(47.99, 7.85), (47.98, 7.90), (47.95, 7.95), (47.93, 8.00), (47.92, 8.05)].enumerated()
            .map { RoutePoint(coordinate: Coordinate(latitude: $1.0, longitude: $1.1), elevationMeters: 300 + 50 * Double($0)) }
        let line = points.map(\.coordinate)
        let source = PlannerTestSource()
        let model = PlannerPreviewModel(plan: try #require(PlannerPlan.keptLine(points)), service: source)
        await model.calculateRoute()
        #expect(Array(model.geometry.dropFirst().dropLast()) == line && !model.canUndo && model.canSave)
        #expect(model.stats.ascentMeters == 200 && model.routePoints.allSatisfy { $0.elevationMeters != nil })
        func split(_ from: Coordinate, _ to: Coordinate) async throws {
            let middle = Coordinate(latitude: (from.latitude + to.latitude) / 2, longitude: (from.longitude + to.longitude) / 2)
            let hit = try #require(model.leg(near: middle, within: 50))
            #expect(model.leg(hit).mode == .drawn)
            model.addPoint(on: hit)
            await model.calculateRoute()
        }
        try await split(line[0], line[1])
        try await split(line[2], line[3])
        #expect(model.points.map(\.kind) == [.visit, .shape, .shape, .visit])
        #expect(Array(Set(model.geometry)).count == line.count + 2 && model.stats.ascentMeters == 200)
        #expect(await source.requests.isEmpty)

        let moved = Coordinate(latitude: 47.94, longitude: 7.99)
        model.replacePoint(id: model.points[2].id, with: .init(id: "moved", name: "Moved", coordinate: moved))
        await model.calculateRoute()
        #expect(await source.requests == [[model.points[1].place.coordinate, moved, line[4]]])
        #expect(model.exportPlan().routePoints.map(\.leg) == [nil, .drawn, nil, nil])
    }

    @Test func theWaypointsOfAKeptLineAreMarkersAndExportAgain() async throws {
        let line = [(47.99, 7.85), (47.97, 7.95), (47.93, 8.00)].map { RoutePoint(coordinate: Coordinate(latitude: $0.0, longitude: $0.1)) }
        let spring = Waypoint(index: 0, name: "Spring", note: "Cold all year", distanceAlongMeters: 0,
                              coordinate: Coordinate(latitude: 47.975, longitude: 7.93), category: .water)
        let hut = Waypoint(index: 1, name: "Hut", distanceAlongMeters: 0, coordinate: Coordinate(latitude: 47.94, longitude: 7.99),
                           category: .accommodation)
        let model = PlannerPreviewModel(plan: try #require(PlannerPlan.keptLine(line, waypoints: [hut, spring])), service: PlannerTestSource())
        await model.calculateRoute()
        #expect(model.points.count == 2 && model.markers.map(\.place.name) == ["Hut", "Spring"])
        let waypoints = model.exportRoute(name: "Ride").waypoints
        #expect(waypoints.map(\.name) == ["Spring", "Hut"] && waypoints.map(\.category) == [.water, .accommodation])
        #expect(waypoints.map(\.index) == [0, 1] && waypoints[0].distanceAlongMeters < waypoints[1].distanceAlongMeters)
        // Off the line, as the import placed it: a signed offset and the note stay.
        #expect(waypoints[0].note == "Cold all year" && (-200 ... -50).contains(waypoints[0].lateralOffsetMeters))
        #expect(model.exportPlan().markers.first { $0.label == "Spring" }?.note == "Cold all year")
    }

    @Test func aLegWithADrawnLineCanRouteAndGoBack() async throws {
        let line = [(47.99, 7.85), (47.97, 7.95), (47.93, 8.00)].map { Coordinate(latitude: $0.0, longitude: $0.1) }
        let source = PlannerTestSource()
        let model = PlannerPreviewModel(plan: try #require(PlannerPlan.keptLine(line.map { RoutePoint(coordinate: $0) })), service: source)
        await model.calculateRoute()
        let hit = try #require(model.leg(near: line[1], within: 10))
        model.setLegMode(hit, to: .routed)
        await model.calculateRoute()
        #expect(model.geometry == [line[0], line[2]])
        #expect(await source.requests.count == 1)
        #expect(model.leg(near: line[1], within: 10) == nil && model.leg(hit).drawn == [RoutePoint(coordinate: line[1])])
        model.setLegMode(hit, to: .drawn)
        await model.calculateRoute()
        #expect(model.geometry == line)
    }

    @Test func aPlanWithNightsRoundTripsThroughThePlanner() throws {
        let at = { (lon: Double) in Coordinate(latitude: 47.9, longitude: lon) }
        let plan = PlannerPlan(points: [
            PlanPoint(id: "start", label: "Freiburg", coordinate: at(7.8), progress: 0, kind: .start),
            PlanPoint(id: "night-1", label: "Camp", coordinate: at(7.9), progress: 0, kind: .night, night: 1, placeKind: "camping",
                      leg: .drawn, drawn: [RoutePoint(coordinate: at(7.85), elevationMeters: 420)]),
            PlanPoint(id: "v", label: "Shaping point", coordinate: at(7.95), progress: 0, kind: .via, turnaround: true),
            PlanPoint(id: "w", label: "Fountain", coordinate: at(8.0), progress: 0, kind: .waypoint, placeKind: "water"),
            PlanPoint(id: "night-2", label: "Hut", coordinate: at(8.05), progress: 0, kind: .night, night: 2),
            PlanPoint(id: "finish", label: "Titisee", coordinate: at(8.15), progress: 0, kind: .finish),
            PlanPoint(id: "m", label: "View", coordinate: at(8.1), progress: 0, kind: .marker),
        ], mode: .trip, bike: "road", preset: "Shorter", routeOrder: ["night-1", "v", "w", "night-2"])
        let model = PlannerPreviewModel(plan: plan, service: PlannerTestSource())
        #expect(model.dayCount == 3 && model.activity == .road && model.preset == .shorter)
        #expect(model.exportPlan() == plan)
    }

    @Test func onlyTheActivitiesOfTheReleaseAreOffered() async {
        let model = PlannerPreviewModel(sample: true, service: PlannerTestSource())
        #expect(model.activities == RouteActivity.allCases)
        await model.calculateRoute()
        await model.loadActivities()
        #expect(model.activities == [.road, .gravel, .mtb, .touring])
    }

    @Test func theHardestPartReadsInTheGradesOfTheActivity() {
        #expect(PlannerRoutesText.reading(0...2, mtb: false).0 == "Up to T3.")
        #expect(PlannerRoutesText.reading(0...1, mtb: false).1 == "Routes whose hardest part is T1 or T2. A path without a grade counts as T1.")
        #expect(PlannerRoutesText.reading(2...3, mtb: false).0 == "Must include T3 or harder.")
        #expect(PlannerRoutesText.reading(1...2, mtb: true).0 == "Must include S1 or harder, up to S2.")
        #expect(PlannerRoutesText.summary(PlannerRouteFilters(), activity: .hiking) == "10 km · Loop · up to T3")
        #expect(PlannerRoutesText.summary(PlannerRouteFilters(), activity: .touring) == "10 km · Loop")
    }

    @Test func nightsEndDaysAtAnyPointUpToFourteenDays() async throws {
        let at = { (lon: Double) in Coordinate(latitude: 47.9, longitude: lon) }
        let vias = (1...14).map { PlanPoint(id: "v\($0)", label: "Shaping point", coordinate: at(7.8 + 0.01 * Double($0)), progress: 0, kind: .via) }
        let plan = PlannerPlan(points: [PlanPoint(id: "start", label: "A", coordinate: at(7.8), progress: 0, kind: .start)] + vias
            + [PlanPoint(id: "finish", label: "B", coordinate: at(8.0), progress: 1, kind: .finish)], routeOrder: vias.map(\.id))
        let model = PlannerPreviewModel(plan: plan, service: PlannerTestSource())
        for via in vias.prefix(13) { model.setNight(id: via.id, true, name: "Camp \(via.id)") }
        #expect(model.dayCount == 14 && !model.canAddDay)
        model.setNight(id: "v14", true)
        #expect(!model.isNight("v14"), "a plan holds at most 14 days")
        #expect(model.nights.first?.place.name == "Camp v1" && model.nights.allSatisfy { $0.kind == .visit })
        await model.calculateRoute()
        #expect(model.dayStats.count == 14 && model.exportPlan().days == 14 && model.exportPlan().mode == .trip)

        model.movePoint(id: "v1", to: at(7.815))
        #expect(model.isNight("v1") && model.points[1].place.coordinate == at(7.815))
        model.setNight(id: "v1", false)
        #expect(model.dayCount == 13 && model.canAddDay)
        // A new night lands in ride order, not before the finish.
        await model.calculateRoute()
        model.endDay(at: .init(id: "camp", name: "Camp", coordinate: Coordinate(latitude: 47.9005, longitude: 7.8455)))
        #expect(model.points.firstIndex { $0.id == "camp" } == 5 && model.isNight("camp") && model.dayCount == 14)
        model.clearNights()
        #expect(model.dayCount == 1)
    }

    @Test func aTransferLegCountsNoDistanceTimeOrClimbAndDrawsApart() async throws {
        let at = { (lon: Double) in RoutePoint(coordinate: Coordinate(latitude: 47.9, longitude: lon), elevationMeters: 400) }
        let file = { (from: Double, to: Double) in stride(from: from, through: to, by: 0.01).map(at) }
        let trip = Trip.joining([file(7.80, 7.84), file(7.90, 7.94)], id: TripID("t"), name: "T", bikeType: .road,
                                now: Date(timeIntervalSince1970: 0))
        let model = PlannerPreviewModel(plan: try #require(PlannerPlan.keptLine(trip)), service: PlannerTestSource())
        await model.calculateRoute()
        let ridden = 2 * MeasuredLine(routePoints: file(7.80, 7.84)).length
        #expect(abs(model.stats.distanceMeters - ridden) < 1 && model.stats.ascentMeters == 0)
        #expect(abs(model.routeLine.length - ridden) < 1 && model.profile.distance == model.routeLine.length,
                "the profile has a gap for the transfer")
        #expect(model.dayStats.count == 2 && abs(model.dayStats.reduce(0) { $0 + $1.distanceMeters } - ridden) < 1)
        #expect(abs(model.dayStats[1].seconds - model.dayStats[0].seconds) < 1, "the transfer adds no time")
        #expect(model.lineRuns.map(\.isTransfer) == [false, true, false])
        // Day 2 starts where the transfer reaches, not at the night.
        #expect(model.dayPlaces.map(\.from.coordinate) == [7.80, 7.90].map { at($0).coordinate })
    }

    @Test func aTransferRunsOnlyFromANight() throws {
        let at = { (lon: Double) in Coordinate(latitude: 47.9, longitude: lon) }
        let plan = PlannerPlan(points: [
            PlanPoint(id: "start", label: "A", coordinate: at(7.80), progress: 0, kind: .start),
            PlanPoint(id: "b", label: "B", coordinate: at(7.82), progress: 0, kind: .waypoint, leg: .transfer),
            PlanPoint(id: "c", label: "C", coordinate: at(7.84), progress: 0, kind: .waypoint),
            PlanPoint(id: "finish", label: "D", coordinate: at(7.90), progress: 1, kind: .finish),
        ], routeOrder: ["b", "c"])
        let model = PlannerPreviewModel(plan: plan, service: PlannerTestSource())
        #expect(model.exportPlan().routePoints.map(\.leg) == [nil, nil, nil, nil], "inside a day a transfer is routed")
        let leg = try #require(model.leg(into: "c"))
        #expect(!model.legModes(leg).contains(.transfer))
        model.setLegMode(leg, to: .transfer)
        #expect(model.leg(leg).mode == .routed)

        model.setNight(id: "b", true)
        #expect(model.legModes(leg).contains(.transfer))
        model.setLegMode(leg, to: .transfer)
        #expect(model.leg(leg).mode == .transfer)
        model.setNight(id: "b", false)
        #expect(model.leg(leg).mode == .routed, "the night goes, and its transfer with it")
    }

    @Test func aDraggedPointPlansOnlyItsTwoLegsAndATransferStaysOne() async throws {
        let at = { (lon: Double) in Coordinate(latitude: 47.9, longitude: lon) }
        let plan = PlannerPlan(points: [
            PlanPoint(id: "start", label: "A", coordinate: at(7.80), progress: 0, kind: .start),
            PlanPoint(id: "b", label: "B", coordinate: at(7.82), progress: 0, kind: .waypoint, leg: .drawn, drawn: [RoutePoint(coordinate: at(7.81))]),
            PlanPoint(id: "c", label: "C", coordinate: at(7.84), progress: 0, kind: .waypoint, leg: .drawn, drawn: [RoutePoint(coordinate: at(7.83))]),
            PlanPoint(id: "night-1", label: "D", coordinate: at(7.86), progress: 0, kind: .night, night: 1, leg: .drawn,
                      drawn: [RoutePoint(coordinate: at(7.85))]),
            PlanPoint(id: "finish", label: "E", coordinate: at(7.90), progress: 1, kind: .finish, leg: .transfer),
        ], routeOrder: ["b", "c", "night-1"])
        let source = PlannerTestSource()
        let model = PlannerPreviewModel(plan: plan, service: source)
        await model.calculateRoute()
        #expect(await source.requests.isEmpty)

        let moved = Coordinate(latitude: 47.91, longitude: 7.84)
        model.movePoint(id: "c", to: moved)
        await model.calculateRoute()
        #expect(await source.requests == [[at(7.82), moved, at(7.86)]])
        #expect(model.exportPlan().routePoints.map(\.leg) == [nil, .drawn, nil, nil, .transfer])
        #expect(model.points[2].place.name == PlannerPreviewModel.mapPointName && model.points[2].place.kind == .town)

        model.movePoint(id: "night-1", to: Coordinate(latitude: 47.91, longitude: 7.87))
        #expect(model.exportPlan().routePoints.map(\.leg) == [nil, .drawn, nil, nil, .transfer], "a moved transfer stays one")
        model.undo(); model.undo()
        #expect(model.points.map(\.place.coordinate) == [7.80, 7.82, 7.84, 7.86, 7.90].map(at))
    }
}

private actor PlannerTestSource: PlannerDataSource {
    /// The points and turnarounds of each route request.
    var requests: [[Coordinate]] = []
    var turnarounds: [[Int]] = []
    func release() async throws -> PlannerRelease { testRelease }
    func profiles(release: PlannerRelease) -> [String]? { ["gravel", "gravel/shorter", "mtb", "road", "touring"] }
    func route(points: [Coordinate], turnarounds: [Int], activity: RouteActivity, preference: RoutePreference, release: PlannerRelease) async throws -> PlannedPath {
        requests.append(points)
        self.turnarounds.append(turnarounds)
        let samples = points.map { RoutePoint(coordinate: $0, elevationMeters: 300) }
        let length = MeasuredLine(routePoints: samples).length
        let elapsed = MeasuredLine(routePoints: samples).vertices.map { $0.distance / 4 }
        return PlannedPath(points: samples, distance: length, ascent: 0, seconds: length / 4,
                           pointIndices: Array(points.indices), elapsed: elapsed)
    }
    func search(_ query: PlannerSearchQuery, release: PlannerRelease) async throws -> [PlannerPlace] { [] }
}

private actor ControlledPlannerSource: PlannerDataSource {
    private var requests: [[Coordinate]] = []
    private var replies: [Int: CheckedContinuation<PlannedPath, any Error>] = [:]
    private var waiters: [(Int, CheckedContinuation<Void, Never>)] = []
    func release() async throws -> PlannerRelease { testRelease }
    func route(points: [Coordinate], turnarounds: [Int], activity: RouteActivity, preference: RoutePreference, release: PlannerRelease) async throws -> PlannedPath {
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
}

private let testRelease: PlannerRelease = try! JSONDecoder().decode(PlannerRelease.self, from: Data("""
{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","region":"test","bounds":[7,47,9,49],"basemap":"https://test/basemap.json","glyphs":"https://test/fonts/{fontstack}/{range}.pbf","sprites":"https://test/sprites","terrain":"https://test/{z}/{x}/{y}.webp","terrain_attribution":"Terrain","search":"https://test/search","routing":"https://test/routing","manifest":"https://test/manifest.json"}
""".utf8))
#endif
