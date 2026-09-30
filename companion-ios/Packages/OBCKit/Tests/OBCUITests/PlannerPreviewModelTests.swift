#if DEBUG
import Testing
import Foundation
import OBCDomain
@testable import OBCUI

@MainActor
struct PlannerPreviewModelTests {
    @Test func previewAndApplyAreSeparateAndNewRouteIsOneUndoStep() throws {
        let model = PlannerPreviewModel()
        let query = model.lookup("Freiburg to Titisee")
        #expect(!model.hasRoute && !model.canUndo)
        model.apply(try #require(query.action))
        #expect(model.hasRoute && model.geometry.count > 100)
        model.addPoint(PlannerPreviewModel.sampleMapPlaces[2])
        model.apply(.splitDays)
        let geometry = model.geometry
        let orderedPoints = model.points
        let export = model.exportRoute(name: "Black Forest")
        #expect(export.name == "Preview · Black Forest")
        #expect(export.creator == "OpenBikeComputer interaction preview")
        #expect(export.points.map(\.coordinate) == geometry && export.waypoints.count == 2)
        model.newRoute()
        #expect(!model.hasRoute && model.geometry.isEmpty && model.points.isEmpty && model.overnight == nil)
        model.undo()
        #expect(model.geometry == geometry && model.points == orderedPoints && model.dayCount == 2)
        model.redo()
        #expect(!model.hasRoute)
        model.setStart(PlannerPreviewModel.sampleMapPlaces[0])
        #expect(!model.canRedo)
    }

    @Test func daySplitAndReversePreserveTheWholeRoute() {
        let model = PlannerPreviewModel(sample: true)
        model.addPoint(PlannerPreviewModel.sampleMapPlaces[2])
        model.addPoint(PlannerPreviewModel.sampleMapPlaces[3])
        model.apply(.splitDays)
        let distance = model.stats.distanceMeters
        #expect(model.dayStats.count == 2)
        #expect(abs(model.dayStats.reduce(0) { $0 + $1.distanceMeters } - distance) < 0.01)
        #expect(model.dayStats.allSatisfy { $0.distanceMeters > 0 })
        let points = model.points
        model.apply(.reverse)
        #expect(model.start?.id == "titisee" && model.finish?.id == "freiburg")
        #expect(model.points == Array(points.reversed()))
        #expect(abs(model.stats.distanceMeters - distance) < 5)
        model.undo()
        #expect(model.start?.id == "freiburg" && model.dayCount == 2 && model.points == points)
        model.setOvernight(nil)
        #expect(model.dayCount == 1 && model.overnightProgress == nil)
    }

    @Test func unsupportedQueriesAndDuplicateVisitsDoNotChangeThePlan() {
        let model = PlannerPreviewModel(sample: true)
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
        let model = PlannerPreviewModel(sample: true)
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
        model.removePoint(id: "titisee")
        #expect(model.start?.id == "water" && model.finish == nil && !model.hasRoute)
        model.addPoint(cafe)
        #expect(Set(model.points.map(\.id)).count == model.points.count)
    }

    @Test func markersLeaveTheLineAloneAndShapesDoNotExportStops() {
        let model = PlannerPreviewModel(sample: true)
        let cafe = PlannerPreviewModel.sampleMapPlaces[2]
        let initial = model.routePoints
        model.addPoint(cafe, kind: .marker)
        #expect(model.routePoints == initial && model.markers.count == 1)
        model.setPointKind(id: cafe.id, kind: .shape)
        let shaped = model.routePoints
        #expect(shaped != initial && model.markers.isEmpty && model.points.count == 3)
        #expect(model.exportRoute(name: "Ride").waypoints.isEmpty)
        model.setPointKind(id: cafe.id, kind: .visit)
        #expect(model.routePoints == shaped && model.exportRoute(name: "Ride").waypoints.count == 1)
        #expect(model.routePoints.map(\.coordinate) == model.geometry)
    }

    @Test func overnightMovesWithItsPointAndEndpointMovesClearItAtomically() throws {
        let model = PlannerPreviewModel(sample: true)
        model.addPoint(PlannerPreviewModel.sampleMapPlaces[2])
        model.addPoint(PlannerPreviewModel.sampleMapPlaces[3])
        model.apply(.splitDays)
        let id = try #require(model.overnightPointID)
        model.movePoint(fromOffsets: IndexSet(integer: 3), toOffset: 1)
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
}
#endif
