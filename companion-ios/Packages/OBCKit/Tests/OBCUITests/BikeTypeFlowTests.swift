import Foundation
import Testing
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The last-used bike type: an import starts with it, and a route's type change sets it.
@MainActor @Suite struct BikeTypeFlowTests {
    private func freshStore() throws -> LastBikeTypeStore {
        let suite = "obc.test.bikeType.\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defaults.removePersistentDomain(forName: suite)
        return LastBikeTypeStore(defaults: defaults)
    }

    private let route = ImportedRoute(
        name: "Col Loop",
        points: [
            RoutePoint(coordinate: Coordinate(latitude: 48.0, longitude: 8.0), elevationMeters: 500),
            RoutePoint(coordinate: Coordinate(latitude: 48.3, longitude: 8.2), elevationMeters: 900),
        ]
    )

    @Test
    func anImportGetsTheLastUsedType() throws {
        let lastBikeType = try freshStore()
        lastBikeType.value = .gravel
        let flow = ImportFlowModel(
            decode: { [route] _, _ in route }, library: InMemoryLibraryStore(),
            isBonded: { true }, lastBikeType: lastBikeType)

        flow.open(data: Data("<gpx/>".utf8), fileName: "col.gpx")
        let pending = try #require(flow.pendingImport)
        let landing = RouteDetailModel(
            transport: MockTransport(control: MockControl(scenario: .happyPath)),
            dressing: .imported(pending.route, fileName: pending.fileName), bikeType: pending.bikeType)

        let record = pending.record(for: landing.makeDetail())
        #expect(record.bikeType == .gravel)
        let totals = try #require(RouteObjectCodec.totals(points: route.points))
        #expect(record.summary.estimatedDuration == TimeInterval(BikeType.gravel.estimatedSeconds(
            distanceMeters: totals.distanceMeters, ascentMeters: totals.ascentMeters)))
    }

    @Test
    func changingARoutesTypeSetsTheLastUsedTypeAndItsEstimate() throws {
        let lastBikeType = try freshStore()
        let library = InMemoryLibraryStore()
        let model = MainScreenModel(
            transport: MockTransport(control: MockControl(scenario: .happyPath)),
            library: library, lastBikeType: lastBikeType)
        let summary = RouteSummary(id: RouteID("col"), name: "Col Loop", distanceMeters: 42_000, elevationGainMeters: 900)
        model.addImportedRoute(PlannedRouteRecord(
            summary: summary, route: route, sourceFileName: "col.gpx", sourceFileData: Data()))

        model.setBikeType(summary.id, to: .touring)

        #expect(lastBikeType.value == .touring)
        let saved = try #require(library.plannedRoutes().first)
        #expect(saved.bikeType == .touring)
        #expect(model.routes.first?.estimatedDuration == 10_874)  // floor((36·42000 + 900·22·17) / 170)
    }
}
