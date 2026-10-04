import Foundation
import Testing
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// Editing a saved route in the planner: a route without a plan opens as its kept line, and saved
/// changes replace the line in place while the route keeps its identity and device link.
@MainActor @Suite struct RouteEditModelTests {
    @Test func savedChangesKeepTheRouteAndACopyIsASecondRoute() throws {
        let library: any LibraryStore = InMemoryLibraryStore()
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        let model = MainScreenModel(transport: MockTransport(control: control), library: library)
        let id = RouteID("orig")
        let link = DeviceRouteLink(serial: "OBC-001", storeID: "000000000000000000000000a1b2c3d4", objectID: DeviceObjectID(5))
        let points = [(48.00, 8.0), (48.01, 8.01), (48.02, 8.0)].map {
            RoutePoint(coordinate: Coordinate(latitude: $0.0, longitude: $0.1), elevationMeters: 300)
        }
        model.addImportedRoute(PlannedRouteRecord(
            summary: RouteSummary(id: id, name: "Kettle Loop", distanceMeters: 2_000, elevationGainMeters: 0),
            route: ImportedRoute(name: "Kettle Loop", points: points), bikeType: .touring,
            sourceFileName: "loop.gpx", sourceFileData: Data("<gpx/>".utf8), deviceLink: link, uploadedCRC32: 7))

        let plan = try #require(model.plannedPlan(for: id))
        #expect(plan.name == "Kettle Loop" && plan.bike == "touring")
        #expect(plan.routePoints.map(\.kind) == [.start, .finish] && plan.routePoints[1].leg == .drawn)

        let edited = ImportedRoute(name: "Kettle Loop", points: Array(points.prefix(2)))
        let record = PlannedRouteRecord(
            summary: RouteSummary(id: RouteID("fresh"), name: "Kettle Loop", distanceMeters: 1_400, elevationGainMeters: 0),
            route: edited, bikeType: .touring, sourceFileName: "Kettle Loop.gpx", sourceFileData: Data(), plan: plan)
        model.saveRouteChanges(id, to: record)
        let saved = try #require(library.plannedRoutes().first { $0.id == id })
        #expect(library.plannedRoutes().count == 1 && model.routes.map(\.id) == [id])
        #expect(saved.route == edited && saved.plan == plan && saved.summary.distanceMeters == 1_400)
        #expect(saved.summary.name == "Kettle Loop" && saved.sourceFileName == "loop.gpx")
        #expect(saved.deviceLink == link && saved.uploadedCRC32 == 7)

        model.addImportedRoute(record)
        #expect(model.routes.map(\.id) == [RouteID("fresh"), id] && library.plannedRoutes().count == 2)
    }
}
