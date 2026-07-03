import XCTest
import OBCDomain
@testable import OBCMock

/// Every `Scenario` produces its target screen's transport-observable behavior
/// with no device and no firmware. Pure UI-layer scenarios (`unsupportedFile`,
/// `syncUpToDate`) only assert the happy link + the scenario tag the UI branches on.
final class ScenarioTests: XCTestCase {
    private func transport(_ scenario: Scenario) -> (MockTransport, MockControl) {
        let control = MockControl(scenario: scenario)
        control.latency = .zero   // keep the sweep fast (coldRead asserts the knob, not the wait)
        return (MockTransport(control: control), control)
    }

    func testEveryScenarioConstructsWithoutTrapping() {
        for scenario in Scenario.allCases {
            let control = MockControl(scenario: scenario)
            XCTAssertEqual(control.scenario, scenario)
        }
    }

    func testHappyPath() async throws {
        let (transport, control) = transport(.happyPath)
        XCTAssertEqual(control.connection, .connected)
        let routes = try await transport.listRoutes()
        let rides = try await transport.listRides()
        XCTAssertFalse(routes.isEmpty)
        XCTAssertFalse(rides.isEmpty)
    }

    func testEmptyLibrary() async throws {
        let (transport, _) = transport(.emptyLibrary)
        let routes = try await transport.listRoutes()
        XCTAssertTrue(routes.isEmpty)
    }

    func testColdReadArmsASlowRead() {
        // Asserted as the latency knob, not by waiting out a real 3s read.
        XCTAssertGreaterThanOrEqual(Scenario.coldRead.preset.latency, .seconds(3))
    }

    func testReadErrorThenRetrySucceeds() async throws {
        let (transport, _) = transport(.readError)
        do {
            _ = try await transport.listRoutes()
            XCTFail("expected S3 read error")
        } catch let error as DeviceError {
            XCTAssertEqual(error, .readFailed)
        }
        let retry = try await transport.listRoutes()
        XCTAssertFalse(retry.isEmpty)                              // retry recovers
    }

    func testOutOfRangeShowsBannerButStillServesCachedContent() async throws {
        let (transport, control) = transport(.outOfRange)
        XCTAssertEqual(control.connection, .outOfRange)
        let cached = try await transport.listRoutes()
        XCTAssertFalse(cached.isEmpty)                            // content still renders
    }

    func testNoDeviceBlocksReads() async {
        let (transport, control) = transport(.noDevice)
        XCTAssertEqual(control.connection, .disconnected)
        do {
            _ = try await transport.listRoutes()
            XCTFail("expected notConnected")
        } catch let error as DeviceError {
            XCTAssertEqual(error, .notConnected)
        } catch {
            XCTFail("unexpected error \(error)")
        }
    }

    func testPairingFailures() async {
        // `.timeout` fails in the un-gated scan (`discover`), `.rejected` in the
        // gated `authenticate` — `connect()` runs both, so both surface here.
        await assertConnectThrows(.pairingTimeout, .deviceNotFound)
        await assertConnectThrows(.pairingRejected, .pairingFailed)
    }

    func testRadioStates() async {
        await assertConnectThrows(.bluetoothOff, .bluetoothUnavailable(.poweredOff))
        await assertConnectThrows(.permissionDenied, .bluetoothUnavailable(.unauthorized))
    }

    func testSyncDropArmsATransferDrop() {
        XCTAssertNotNil(Scenario.syncDrop.preset.dropAtFraction)
        XCTAssertNotNil(Scenario.uploadDrop.preset.dropAtFraction)
    }

    func testUiOnlyScenariosHaveAHappyLink() async throws {
        for scenario in [Scenario.syncUpToDate, .unsupportedFile] {
            let (transport, control) = transport(scenario)
            XCTAssertEqual(control.connection, .connected)
            let routes = try await transport.listRoutes()
            XCTAssertFalse(routes.isEmpty)
        }
    }

    // MARK: helper

    private func assertConnectThrows(_ scenario: Scenario, _ expected: DeviceError,
                                     file: StaticString = #filePath, line: UInt = #line) async {
        let (transport, _) = transport(scenario)
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
