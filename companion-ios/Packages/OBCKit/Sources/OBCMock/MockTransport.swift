#if DEBUG
import Foundation
import OBCDomain
import OBCTransport

/// Debug-only build seam: the whole `OBCMock` module is behind `#if DEBUG`, so this
/// string never reaches a Release binary. An acceptance test greps for this exact value.
public let obcMockBuildMarker = "OBCMock:DEBUG-only"

/// Fixture-backed `DeviceTransport` for Debug builds. It bypasses `BLEChannel` and serves
/// domain objects from fixtures with simulated latency. All live state is in the shared
/// `control` reference, so the debug panel and the tests drive the same instance.
public struct MockTransport: DeviceTransport {
    public let control: MockControl

    public init(control: MockControl = MockControl()) { self.control = control }

    public init(scenario: Scenario) { self.control = MockControl(scenario: scenario) }

    // MARK: Lifecycle

    public var state: AsyncStream<ConnectionState> { control.stateMulticast.stream() }
    public var battery: AsyncStream<Int> { control.batteryMulticast.stream() }
    public var catalogChanges: AsyncStream<CatalogChange> {
        // Drop the `nil` seed: local edges are live only.
        let source = control.catalogChangedMulticast.stream()
        return AsyncStream { continuation in
            let pump = Task {
                for await value in source {
                    if let value { continuation.yield(value) }
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in pump.cancel() }
        }
    }

    public func connect() async throws {
        try await discover()
        try await authenticate()
    }

    public func discover() async throws {
        control.connection = .connecting
        await control.delay()
        do {
            try control.radioGate()
            try control.takePendingFailure()
        } catch {
            control.connection = .disconnected
            throw error
        }
    }

    public func authenticate() async throws {
        // The pairing gate stands in for the real path's LESC passkey sheet.
        await control.delay()
        do {
            try control.pairingGate()
        } catch {
            control.connection = .disconnected
            throw error
        }
        control.connection = .connected
    }

    public func disconnect() async { control.connection = .disconnected }

    // MARK: Control plane

    public func deviceInfo() async throws -> DeviceInfo {
        try await preludeThrowing()
        return control.deviceInfo
    }

    public func readConfig() async throws -> DeviceConfig {
        try await preludeThrowing()
        return control.fixtures.config
    }

    public func writeConfig(_ config: DeviceConfig) async throws {
        try await preludeThrowing()
        control.setConfig(config)
    }

    public func readDiagnostics() async throws -> Data {
        try await preludeThrowing()
        return control.fixtures.diagnostics
    }

    // MARK: Data plane

    public func listRoutes() async throws -> [RouteCatalogEntry] {
        // The device catalog under device object ids: reconcile input for the "on device"
        // badge, not for list rows.
        try await preludeThrowing()
        control.recordCancelledRouteCatalogReadIfNeeded()
        return control.deviceRoutes()
    }

    public func deleteRoute(_ id: DeviceObjectID) async throws {
        try await preludeThrowing()
        control.recordRouteObjectDelete(id)
        control.removeRoute(id)
    }

    // MARK: Trips

    public func listTrips() async throws -> [TripCatalogEntry] {
        // Reconcile input for the trip card badge, not for list rows.
        try await preludeThrowing()
        try control.takeTripCatalogFailure()
        return control.deviceTripCatalog()
    }

    public func downloadTrip(_ id: DeviceObjectID) async throws -> TripObjectCodec.Decoded {
        try await preludeThrowing()
        guard let decoded = control.deviceTripDecoded(id) else { throw DeviceError.readFailed }
        return decoded
    }

    public func uploadTrip(_ trip: TripBlob) -> TransferHandle {
        control.beginTripUpload(trip)
    }

    public func deleteTrip(_ id: DeviceObjectID) async throws {
        try await preludeThrowing()
        control.removeTrip(id)
    }

    public func listRides() async throws -> RideCatalog {
        try await preludeThrowing()
        return RideCatalog(rides: control.fixtures.rides.map(\.summary))
    }

    public func setClock(_ sample: WallClockSample) async throws -> ClockSyncOutcome {
        try await preludeThrowing()
        return control.recordSetClock(sample)
    }
    public func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail {
        try await preludeThrowing()
        guard let entry = control.deviceRouteEntry(id) else {
            throw DeviceError.readFailed
        }
        return entry.detail()
    }

    public func rideDetail(_ id: RideID) async throws -> RideDetail {
        try await preludeThrowing()
        guard let entry = control.fixtures.rides.first(where: { $0.summary.id == id }) else {
            throw DeviceError.readFailed
        }
        return entry.detail()
    }

    public func uploadRoute(_ route: RouteBlob) -> TransferHandle {
        control.beginRouteUpload(route)
    }

    public func downloadRides(_ ids: [RideID]) -> RideDownload {
        control.beginRideDownload(ids)
    }

    public func uploadFirmware(_ container: Data) -> TransferHandle {
        control.beginFirmwareUpload(container)
    }

    public func installFirmware() async throws -> FirmwareInstallResult {
        try await preludeThrowing()
        return control.installFirmware()
    }

    public func forgetBond() async throws {
        // The mock models no device-side bond slot, so the record is the only effect.
        try await preludeThrowing()
        control.recordForgetBond()
    }

    /// Applies latency, requires a reachable link, then honors an armed one-shot failure.
    private func preludeThrowing() async throws {
        await control.delay()
        try control.requireReachable()
        try control.takePendingFailure()
    }
}
#endif
