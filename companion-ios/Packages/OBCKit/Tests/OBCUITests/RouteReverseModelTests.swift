import Foundation
import Testing
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// Reversing a route, the app half: `MainScreenModel.reverseRoute` lands a flipped second route
/// in the library and leaves the original untouched. The geometry transform itself is pinned in
/// `RouteReverseTests`; this pins the model wiring.
@MainActor @Suite struct RouteReverseModelTests {
    private func makeModel() -> (MainScreenModel, any LibraryStore) {
        let library: any LibraryStore = InMemoryLibraryStore()
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        let model = MainScreenModel(transport: MockTransport(control: control), library: library)
        return (model, library)
    }

    /// A climbing route with two waypoints: enough to prove the swap and the waypoint
    /// re-ordering land end to end.
    private func climbingRecord(name: String = "Kettle Loop") -> PlannedRouteRecord {
        let points = [
            RoutePoint(coordinate: Coordinate(latitude: 48.00, longitude: 8.0), elevationMeters: 100),
            RoutePoint(coordinate: Coordinate(latitude: 48.01, longitude: 8.0), elevationMeters: 200),
            RoutePoint(coordinate: Coordinate(latitude: 48.02, longitude: 8.0), elevationMeters: 320),
        ]
        return PlannedRouteRecord(
            summary: RouteSummary(id: RouteID("orig"), name: name,
                                  distanceMeters: 0, elevationGainMeters: 0, source: .gpx),
            route: ImportedRoute(
                name: name, points: points,
                waypoints: [
                    Waypoint(index: 0, name: "Start", distanceAlongMeters: 0,
                             coordinate: Coordinate(latitude: 48.00, longitude: 8.0)),
                    Waypoint(index: 1, name: "Top", distanceAlongMeters: 2200,
                             coordinate: Coordinate(latitude: 48.02, longitude: 8.0)),
                ]),
            sourceFileName: "loop.gpx",
            sourceFileData: Data("<gpx/>".utf8)
        )
    }

    @Test func createsSecondRouteLeavingOriginalUntouched() {
        let (model, _) = makeModel()
        model.addImportedRoute(climbingRecord())

        let newID = model.reverseRoute(RouteID("orig"))
        #expect(newID != nil)
        #expect(newID != RouteID("orig"))

        #expect(model.routes.count == 2)
        #expect(model.routes.contains { $0.id == RouteID("orig") })
        #expect(model.routes.contains { $0.id == newID })

        // The reversed copy leads the list (newest first) with the suffixed name.
        #expect(model.routes.first?.id == newID)
        #expect(model.routes.first?.name == "Kettle Loop (reversed)")
        // The original's name is left as it was.
        let original = model.routes.first { $0.id == RouteID("orig") }
        #expect(original?.name == "Kettle Loop")
    }

    @Test func reversedRouteHasFlippedGeometryAndSwappedClimb() {
        let (model, _) = makeModel()
        model.addImportedRoute(climbingRecord())
        let newID = model.reverseRoute(RouteID("orig"))!

        let reversed = model.plannedGeometry(for: newID)!
        // The reversed route starts at the old summit.
        #expect(reversed.points.first?.coordinate == Coordinate(latitude: 48.02, longitude: 8.0))
        #expect(reversed.points.last?.coordinate == Coordinate(latitude: 48.00, longitude: 8.0))
        // "Top" was near the end and now leads; "Start" was at 0 and is now furthest along.
        #expect(reversed.waypoints.map(\.name) == ["Top", "Start"])
        #expect(reversed.waypoints[0].distanceAlongMeters < reversed.waypoints[1].distanceAlongMeters)

        // The stats are re-derived from the reversed geometry: the forward climb is now a pure
        // descent, so the gain is zero.
        let summary = model.routes.first { $0.id == newID }!
        #expect(summary.elevationGainMeters == 0)
        #expect(summary.distanceMeters > 0)
        #expect(summary.pointCount == 3)
        #expect(summary.source == .gpx)  // wire lineage carried through
    }

    @Test func reverseOfMissingRouteReturnsNil() {
        let (model, _) = makeModel()
        #expect(model.reverseRoute(RouteID("nope")) == nil)
    }

    @Test func reversedRouteIsNotYetOnTheDevice() {
        let (model, _) = makeModel()
        model.addImportedRoute(climbingRecord())
        let newID = model.reverseRoute(RouteID("orig"))!
        // A fresh library route has no device link until it is uploaded.
        #expect(model.onDeviceState(newID) == .notOnDevice)
    }
}
