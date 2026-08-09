import XCTest
import OBCDomain
import OBCTransport
@testable import OBCMock

/// Tests run in Debug, so `#if DEBUG` is active and `OBCMock` is non-empty here.
/// The transport surface: it serves fixtures, honors latency, and is stateful
/// (delete/rename persist). All use a zero-latency control so the suite stays fast.
final class MockTransportTests: XCTestCase {
    private func fastControl(_ scenario: Scenario = .happyPath) -> MockControl {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        return control
    }

    func testDebugBuildMarkerIsCompiledIn() {
        XCTAssertEqual(obcMockBuildMarker, "OBCMock:DEBUG-only")
    }

    func testServesFixtureDeviceInfo() async throws {
        let transport = MockTransport(control: fastControl())
        let info = try await transport.deviceInfo()
        XCTAssertEqual(info.name, "Trailhead")
        XCTAssertEqual(info.firmwareVersion, "0.4.2")
        XCTAssertEqual(info.protocolVersion, OBCProtocol.version)
    }

    func testControlOverridesDeviceInfo() async throws {
        let custom = DeviceInfo(name: "OBC #42", firmwareVersion: "1.2.3")
        let transport = MockTransport(control: MockControl(deviceInfo: custom))
        let info = try await transport.deviceInfo()
        XCTAssertEqual(info, custom)
    }

    func testListsTheDeviceHeldRoutesUnderDeviceNamespaceIDs() async throws {
        // `listRoutes` is the device's catalog (reconcile input, #289): exactly
        // the fixture routes with a `deviceObjectID`, listed under that id —
        // never the whole library.
        let transport = MockTransport(control: fastControl())
        let routes = try await transport.listRoutes()
        XCTAssertEqual(Set(routes.map(\.id)), [DeviceObjectID(7), DeviceObjectID(12)])
        XCTAssertTrue(routes.contains { $0.name == "Kettle Moraine Loop" })

        let rides = try await transport.listRides().rides
        XCTAssertTrue(rides.contains { $0.name == "Sunday Coffee Spin" })
        XCTAssertEqual(rides.count, 4)
    }

    func testDeleteRoutePersists() async throws {
        let transport = MockTransport(control: fastControl())
        let before = try await transport.listRoutes()
        let victim = try XCTUnwrap(before.first)
        try await transport.deleteRoute(victim.id)
        let after = try await transport.listRoutes()
        XCTAssertEqual(after.count, before.count - 1)
        XCTAssertFalse(after.contains { $0.id == victim.id })
    }

    func testWriteConfigRenamesDeviceAndPersists() async throws {
        // Delta 1: the device name lives in Config; renaming surfaces in DIS too.
        let transport = MockTransport(control: fastControl())
        try await transport.writeConfig(DeviceConfig(name: "Ridgeline", units: .imperial))
        let config = try await transport.readConfig()
        let info = try await transport.deviceInfo()
        XCTAssertEqual(config.name, "Ridgeline")
        XCTAssertEqual(config.units, .imperial)
        XCTAssertEqual(info.name, "Ridgeline")
    }

    /// Spec §11.8, write direction: an absent refresh field means **leave the stored value
    /// alone**, not "reset to the default". A rename is exactly the write that carries no refresh
    /// byte, so a device that treated absence as a choice would switch a rider who deliberately
    /// picked `Off` back to 30-minute wakeups every time they renamed it.
    func testWriteConfigWithoutARefreshFieldLeavesTheStoredIntervalAlone() async throws {
        let transport = MockTransport(control: fastControl())
        try await transport.writeConfig(DeviceConfig(name: "Ridgeline", weatherRefresh: .off))
        let stored = try await transport.readConfig()
        XCTAssertEqual(stored.knownWeatherRefresh, .off)

        // …now the rename an app predating WX3 writes: name only, no refresh byte.
        try await transport.writeConfig(DeviceConfig(name: "Alpine"))
        let after = try await transport.readConfig()
        XCTAssertEqual(after.name, "Alpine")
        XCTAssertEqual(after.knownWeatherRefresh, .off, "the rename must not re-enable weather")
    }

    /// The other half of §11.8: the one direction that is strict. A device cannot honour an
    /// interval it does not know, and storing anything else would report a setting back to the
    /// rider that was in fact discarded.
    func testWriteConfigRefusesAnIntervalTheDeviceCannotHonour() async throws {
        let transport = MockTransport(control: fastControl())
        let before = try await transport.readConfig()
        do {
            try await transport.writeConfig(DeviceConfig(name: "Ridgeline", weatherRefreshRaw: 200))
            XCTFail("a device asked to adopt an unknown interval must refuse")
        } catch {
            XCTAssertEqual(error as? WeatherRequestError, .unknownRefresh(200))
        }
        let after = try await transport.readConfig()
        XCTAssertEqual(after, before, "and it stores nothing at all")
    }

    func testReadDiagnosticsReturnsFixtureBlob() async throws {
        let transport = MockTransport(control: fastControl())
        let text = String(decoding: try await transport.readDiagnostics(), as: UTF8.self)
        XCTAssertTrue(text.contains("OBC diagnostics"))
    }

    func testConnectDrivesStateToConnected() async throws {
        let control = fastControl(.noDevice)  // starts disconnected
        let transport = MockTransport(control: control)
        try await transport.connect()
        XCTAssertEqual(control.connection, .connected)
    }

    func testLatencyIsHonored() async throws {
        let control = MockControl(scenario: .happyPath)
        control.latency = .milliseconds(120)
        let transport = MockTransport(control: control)
        let start = ContinuousClock.now
        _ = try await transport.deviceInfo()
        XCTAssertGreaterThanOrEqual(start.duration(to: .now), .milliseconds(100))
    }
}
