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
        control.connection = .connecting
        await control.delay()
        do {
            try control.connectGate()      // radio (H7/H8) + pairing (D5)
            try control.takePendingFailure()
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

    public func listRoutes() async throws -> [RouteSummary] {
        try await preludeThrowing()
        return control.fixtures.routes.map(\.summary)
    }

    public func deleteRoute(_ id: RouteID) async throws {
        try await preludeThrowing()
        control.removeRoute(id)
    }

    public func listRides() async throws -> [RideSummary] {
        try await preludeThrowing()
        return control.fixtures.rides.map(\.summary)
    }

    public func routeDetail(_ id: RouteID) async throws -> RouteDetail {
        try await preludeThrowing()
        guard let entry = control.fixtures.routes.first(where: { $0.summary.id == id }) else {
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
        control.beginTransfer(total: route.payload.count)
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
