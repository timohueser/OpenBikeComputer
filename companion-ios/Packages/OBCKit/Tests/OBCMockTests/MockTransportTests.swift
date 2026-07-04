import XCTest
import OBCDomain
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

        let rides = try await transport.listRides()
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
