import Foundation
import OBCDomain
import OBCMock
import OBCTransport

/// Test-only observation around `MockTransport`. It keeps model tests on the real mock while
/// replacing scheduler delays with explicit operation boundaries.
final class ObservedMockTransport: DeviceLink, DeviceBattery, DeviceObjects, DeviceClock,
    @unchecked Sendable
{
    private let base: MockTransport
    private let gateFirstRouteCatalog: Bool
    private let firstRouteCatalogRelease = AsyncPromise<Void>()
    private let lock = NSLock()
    private var routeCatalogStarted = 0
    private var routeCatalogCompleted = 0
    private var rideDetailStarted = 0
    private var rideDetailCompleted = 0
    private var observedCatalogChanges = false

    init(control: MockControl, gateFirstRouteCatalog: Bool = false) {
        base = MockTransport(control: control)
        self.gateFirstRouteCatalog = gateFirstRouteCatalog
    }

    var routeCatalogStartedCount: Int { lock.withLock { routeCatalogStarted } }
    var routeCatalogCompletedCount: Int { lock.withLock { routeCatalogCompleted } }
    var rideDetailStartedCount: Int { lock.withLock { rideDetailStarted } }
    var rideDetailCompletedCount: Int { lock.withLock { rideDetailCompleted } }
    var catalogChangesObserved: Bool { lock.withLock { observedCatalogChanges } }

    func releaseFirstRouteCatalog() {
        firstRouteCatalogRelease.fulfill(())
    }

    var state: AsyncStream<ConnectionState> { base.state }
    var battery: AsyncStream<Int> { base.battery }
    var catalogChanges: AsyncStream<CatalogChange> {
        lock.withLock { observedCatalogChanges = true }
        return base.catalogChanges
    }

    func connect() async throws { try await base.connect() }
    func disconnect() async { await base.disconnect() }
    func deviceInfo() async throws -> DeviceInfo { try await base.deviceInfo() }
    func setClock(_ sample: WallClockSample) async throws -> ClockSyncOutcome {
        try await base.setClock(sample)
    }

    func listRoutes() async throws -> [RouteCatalogEntry] {
        let call = lock.withLock {
            routeCatalogStarted += 1
            return routeCatalogStarted
        }
        if gateFirstRouteCatalog, call == 1 {
            await firstRouteCatalogRelease.value
        }
        defer { lock.withLock { routeCatalogCompleted += 1 } }
        return try await base.listRoutes()
    }

    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail {
        try await base.routeDetail(id)
    }

    func uploadRoute(_ route: RouteBlob) -> TransferHandle { base.uploadRoute(route) }
    func deleteRoute(_ id: DeviceObjectID) async throws { try await base.deleteRoute(id) }
    func listTrips() async throws -> [TripCatalogEntry] { try await base.listTrips() }
    func downloadTrip(_ id: DeviceObjectID) async throws -> TripObjectCodec.Trip {
        try await base.downloadTrip(id)
    }
    func uploadTrip(_ trip: TripBlob) -> TransferHandle { base.uploadTrip(trip) }
    func deleteTrip(_ id: DeviceObjectID) async throws { try await base.deleteTrip(id) }
    func listRides() async throws -> RideCatalog { try await base.listRides() }

    func rideDetail(_ id: RideID) async throws -> RideDetail {
        lock.withLock { rideDetailStarted += 1 }
        defer { lock.withLock { rideDetailCompleted += 1 } }
        return try await base.rideDetail(id)
    }

    func downloadRides(_ ids: [RideID]) -> RideDownload { base.downloadRides(ids) }
}
