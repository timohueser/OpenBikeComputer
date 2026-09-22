import Foundation
import Testing
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// Save as route and the shared file name, the app half. The ride-to-route transform itself is
/// pinned in `RideToRouteTests`.
@MainActor @Suite struct RideShareModelTests {
    @Test func saveRideAsRouteLandsANewPlannedRouteNamedAfterTheRide() {
        let library: any LibraryStore = InMemoryLibraryStore()
        let control = MockControl(scenario: .happyPath)
        let model = MainScreenModel(transport: MockTransport(control: control), library: library)
        let points = [
            RidePoint(timestamp: Date(), coordinate: Coordinate(latitude: 48.00, longitude: 8.0), elevationMeters: 100),
            RidePoint(timestamp: Date(), coordinate: Coordinate(latitude: 48.01, longitude: 8.0), elevationMeters: 200),
        ]
        let ride = Ride(
            summary: RideSummary(id: RideID("r"), name: "Lunch / Loop", date: Date(), distanceMeters: 1_112),
            points: points
        )

        let id = model.saveRideAsRoute(ride, gpx: Data("<gpx/>".utf8))

        #expect(model.routes.first?.id == id)
        #expect(model.routes.first?.name == "Lunch / Loop")
        #expect(model.tab == .planned)
        let record = library.plannedRoutes().first { $0.id == id }
        #expect(record?.route.points.count == 2)
        #expect(record?.summary.elevationGainMeters == 100)
        #expect(record?.summary.estimatedDuration != nil)
        #expect(record?.sourceFileName == "Lunch - Loop.gpx")
    }

    @Test func gpxFileNameIsCleanedForTheFileSystem() {
        #expect(RideGPXFile.fileName(for: "Furka: day 2/3") == "Furka- day 2-3.gpx")
        #expect(RideGPXFile.fileName(for: " .hidden ") == "hidden.gpx")
        #expect(RideGPXFile.fileName(for: "  ") == "Ride.gpx")
    }
}
