#if DEBUG
import Foundation
import OBCDomain
import OBCTransport

/// Build-seam marker. This symbol exists **only in Debug builds** — the entire
/// `OBCMock` module is behind `#if DEBUG`, so a Release build compiles it to nothing
/// and the string never reaches the Release binary. B0's acceptance test greps the
/// built binary for this exact value (see companion-ios/CLAUDE.md → "Prove the seam").
public let obcMockBuildMarker = "OBCMock:DEBUG-only"

/// Fixture-backed `DeviceTransport` — the default Debug transport (no BLE in the
/// simulator). It **bypasses `BLEChannel` entirely**, serving domain objects straight
/// from fixtures with simulated latency + throughput, driven by a live `MockControl`.
///
/// A thin value type: all live state lives in the shared `control` (a reference), so
/// the debug panel (B1P) and tests manipulate the same instance this transport reads.
public struct MockTransport: DeviceTransport {
    public let control: MockControl

    public init(control: MockControl = MockControl()) { self.control = control }

    /// Convenience: a transport wired to a fresh control for `scenario`.
    public init(scenario: Scenario) { self.control = MockControl(scenario: scenario) }

    // MARK: Lifecycle

    public var state: AsyncStream<ConnectionState> { control.stateMulticast.stream() }
    public var battery: AsyncStream<Int> { control.batteryMulticast.stream() }

    public func connect() async throws {
        // The full link = both phases (bonded reconnect + the direct-connect tests).
        try await discover()
        try await authenticate()
    }

    public func discover() async throws {
        // Phase 1 (#297): the un-gated surface. Radio/scan gate only (H7/H8 +
        // `.timeout`); the pairing decline (D5 rejected) waits for `authenticate()`,
        // so the D2 row appears before any passkey is modelled.
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
        // Phase 2 (#297): the gated ops. The pairing gate stands in for the real
        // path's LESC passkey sheet, fired by the D2 row tap (`confirmPairing`).
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
        // The device's catalog under device object ids — reconcile input for
        // the "on device" badge, never list rows (the Planned list is
        // library-first, #289). Mirrors the real `routeList` download.
        try await preludeThrowing()
        return control.deviceRoutes()
    }

    public func deleteRoute(_ id: DeviceObjectID) async throws {
        try await preludeThrowing()
        control.removeRoute(id)
    }

    public func listRides() async throws -> [RideSummary] {
        try await preludeThrowing()
        return control.fixtures.rides.map(\.summary)
    }

    public func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail {
        // A device object id, exactly like the real transport ("download the
        // route object"). Library-saved routes answer E2 from their own record
        // (`preloadedDetail`), not from here.
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

    // MARK: Shared op prelude

    /// Every control-plane / list op: apply latency, require a reachable link, then
    /// honor an armed one-shot failure. Keeps the per-op bodies to a single line.
    private func preludeThrowing() async throws {
        await control.delay()
        try control.requireReachable()
        try control.takePendingFailure()
    }
}
#endif
