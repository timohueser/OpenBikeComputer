#if DEBUG
import Foundation
import OBCDomain
import OBCTransport

/// Build-seam marker. This symbol exists **only in Debug builds** — the entire
/// file is behind `#if DEBUG`, so a Release build compiles it to nothing and the
/// string never reaches the Release binary. B0's acceptance test greps the built
/// binary for this exact value (see companion-ios/CLAUDE.md → "Prove the seam").
public let obcMockBuildMarker = "OBCMock:DEBUG-only"

/// Fault-injection surface. **B1M** ([#238]) grows this into the full
/// scenario/latency/error control that reproduces every design state on demand;
/// `B1` keeps just enough to back the protocol with sensible fixtures.
public struct MockControl: Sendable {
    public var deviceInfo: DeviceInfo

    public init(
        deviceInfo: DeviceInfo = DeviceInfo(name: "OBC (mock)", firmwareVersion: "0.0.0-mock")
    ) {
        self.deviceInfo = deviceInfo
    }
}

/// Fixture-backed `DeviceTransport` — the default Debug transport (no BLE in the
/// simulator). It **bypasses `BLEChannel` entirely** and serves domain objects
/// straight from fixtures.
///
/// `B1` implements the full protocol as **minimal, honest stubs** so the app
/// compiles and runs against the finalized `DeviceTransport`; the fixtures,
/// `MockControl` fault injection, and per-scenario data are **B1M's** deliverables.
public struct MockTransport: DeviceTransport {
    public let control: MockControl
    private let stateMulticast = AsyncMulticast<ConnectionState>(.connected)
    private let batteryMulticast = AsyncMulticast<Int>(72)

    public init(control: MockControl = MockControl()) {
        self.control = control
    }

    // Lifecycle
    public var state: AsyncStream<ConnectionState> { stateMulticast.stream() }
    public func connect() async throws { stateMulticast.send(.connected) }
    public func disconnect() async { stateMulticast.send(.disconnected) }

    // Control plane
    public func deviceInfo() async throws -> DeviceInfo { control.deviceInfo }
    public var battery: AsyncStream<Int> { batteryMulticast.stream() }
    public func readConfig() async throws -> DeviceConfig { DeviceConfig(name: control.deviceInfo.name) }
    public func writeConfig(_ config: DeviceConfig) async throws {}

    // Data plane (B1M supplies fixtures + fault injection)
    public func listRoutes() async throws -> [RouteSummary] { [] }
    public func uploadRoute(_ route: RouteBlob) -> TransferHandle { .immediatelyFinished() }
    public func deleteRoute(_ id: RouteID) async throws {}
    public func listRides() async throws -> [RideSummary] { [] }
    public func downloadRides(_ ids: [RideID]) -> TransferHandle { .immediatelyFinished() }
    public func readDiagnostics() async throws -> Data { Data() }
}
#endif
