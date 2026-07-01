#if DEBUG
import OBCDomain
import OBCTransport

/// Build-seam marker. This symbol exists **only in Debug builds** — the entire
/// file is behind `#if DEBUG`, so a Release build compiles it to nothing and the
/// string never reaches the Release binary. B0's acceptance test greps the built
/// binary for this exact value (see companion-ios/CLAUDE.md → "Prove the seam").
public let obcMockBuildMarker = "OBCMock:DEBUG-only"

/// Fault-injection surface. B1M turns this into the full scenario/latency/error
/// control that reproduces every design state on demand; B0 keeps just enough to
/// back `fetchDeviceInfo()`.
public struct MockControl: Sendable {
    public var deviceInfo: DeviceInfo

    public init(
        deviceInfo: DeviceInfo = DeviceInfo(name: "OBC (mock)", firmwareVersion: "0.0.0-mock")
    ) {
        self.deviceInfo = deviceInfo
    }
}

/// Fixture-backed `DeviceTransport`. The default Debug transport (no BLE in the
/// simulator). Serves domain objects straight from `MockControl`.
public struct MockTransport: DeviceTransport {
    public let control: MockControl

    public init(control: MockControl = MockControl()) {
        self.control = control
    }

    public func fetchDeviceInfo() async throws -> DeviceInfo {
        control.deviceInfo
    }
}
#endif
