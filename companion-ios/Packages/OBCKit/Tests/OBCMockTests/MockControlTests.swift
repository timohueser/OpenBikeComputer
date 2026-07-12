import XCTest
import OBCDomain
@testable import OBCMock

/// The live fault-injection surface: forced states, one-shot faults, radio/pairing
/// gates, and mid-session event injection — driven programmatically (feeds B1P + UI
/// suites). One shared control instance drives both the panel and the transport.
final class MockControlTests: XCTestCase {
    private func fastControl(_ scenario: Scenario = .happyPath) -> MockControl {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        return control
    }

    func testControlIsSharedLiveBetweenTransports() {
        // Reference semantics: two transports over one control see the same state.
        let control = fastControl()
        let a = MockTransport(control: control)
        let b = MockTransport(control: control)
        control.battery = 41
        XCTAssertEqual(a.control.battery, 41)
        XCTAssertEqual(b.control.battery, 41)
    }

    func testForcingConnectionPushesToStateStreamLive() async {
        let control = fastControl()  // connected
        let transport = MockTransport(control: control)
        let probe = Task { await awaitState(transport.state, equals: .outOfRange) }
        control.connection = .outOfRange   // force mid-session → S4 banner
        let saw = await probe.value
        XCTAssertTrue(saw)
    }

    func testBatteryNudgePushesLive() async {
        let control = fastControl()
        let transport = MockTransport(control: control)
        let probe = Task { await awaitBattery(transport.battery, equals: 23) }
        control.emit(.batteryChanged(23))
        let pushed = await probe.value
        XCTAssertTrue(pushed)
        XCTAssertEqual(control.battery, 23)
    }

    func testFailNextOpIsOneShotThenRecovers() async throws {
        let control = fastControl()
        let transport = MockTransport(control: control)
        control.failNextOp(.readFailed)
        do {
            _ = try await transport.listRoutes()
            XCTFail("expected the armed failure")
        } catch let error as DeviceError {
            XCTAssertEqual(error, .readFailed)   // S3
        }
        // Retry succeeds — the fault was one-shot.
        let routes = try await transport.listRoutes()
        XCTAssertFalse(routes.isEmpty)
    }

    func testRadioOffFailsConnectWithPoweredOff() async {
        let control = fastControl()
        control.setRadio(.off)                 // H8
        let transport = MockTransport(control: control)
        await assertConnectThrows(transport, .bluetoothUnavailable(.poweredOff))
    }

    func testRadioUnauthorizedFailsConnectWithUnauthorized() async {
        let control = fastControl()
        control.setRadio(.unauthorized)        // H7
        let transport = MockTransport(control: control)
        await assertConnectThrows(transport, .bluetoothUnavailable(.unauthorized))
    }

    func testFailPairingTimeoutThrowsOnConnect() async {
        let control = fastControl(.noDevice)
        control.failPairing(.timeout)          // D5
        let transport = MockTransport(control: control)
        await assertConnectThrows(transport, .deviceNotFound)
    }

    func testEmitRideAddedAppearsInNextListRides() async throws {
        let control = fastControl()
        let transport = MockTransport(control: control)
        let before = try await transport.listRides().rides.count
        let newRide = RideSummary(id: RideID("just-now"), name: "Lunch Loop", date: Date(), distanceMeters: 9_000)
        control.emit(.rideAdded(newRide))
        let after = try await transport.listRides().rides
        XCTAssertEqual(after.count, before + 1)
        XCTAssertTrue(after.contains { $0.id == RideID("just-now") })
    }

    func testLoadFixturesSwapsTheSet() async throws {
        let control = fastControl()             // default: 5 routes
        let transport = MockTransport(control: control)
        control.loadFixtures("empty")
        let routes = try await transport.listRoutes()
        XCTAssertTrue(routes.isEmpty)
    }

    func testApplyScenarioResetsKnobs() {
        let control = fastControl()
        control.battery = 5
        control.latency = .seconds(9)
        control.apply(.emptyLibrary)
        XCTAssertEqual(control.scenario, .emptyLibrary)
        XCTAssertEqual(control.battery, 82)            // reset from the empty fixture
        XCTAssertTrue(control.fixtures.routes.isEmpty)
    }

    // MARK: helper

    private func assertConnectThrows(_ transport: MockTransport, _ expected: DeviceError,
                                     file: StaticString = #filePath, line: UInt = #line) async {
        do {
            try await transport.connect()
            XCTFail("expected \(expected)", file: file, line: line)
        } catch let error as DeviceError {
            XCTAssertEqual(error, expected, file: file, line: line)
        } catch {
            XCTFail("unexpected error \(error)", file: file, line: line)
        }
    }
}
