import XCTest
import OBCDomain
@testable import OBCMock

/// The acceptance table: every `Scenario` produces its target screen's
/// transport-observable behavior with no device and no firmware. Pure UI-layer
/// scenarios (`unsupportedFile` H5, `syncUpToDate` H9) only assert the happy link +
/// the scenario tag the UI branches on.
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
        let rides = try await transport.listRides().rides
        XCTAssertFalse(routes.isEmpty)   // C1
        XCTAssertFalse(rides.isEmpty)    // C2
    }

    func testEmptyLibrary() async throws {
        let (transport, _) = transport(.emptyLibrary)
        let routes = try await transport.listRoutes()
        XCTAssertTrue(routes.isEmpty)    // S1
    }

    func testColdReadArmsASlowRead() {
        // S2 skeletons: the preset makes the first read slow (asserted as the knob, not a 3s wait).
        XCTAssertGreaterThanOrEqual(Scenario.coldRead.preset.latency, .seconds(3))
    }

    func testReadErrorThenRetrySucceeds() async throws {
        let (transport, _) = transport(.readError)
        do {
            _ = try await transport.listRoutes()
            XCTFail("expected S3 read error")
        } catch let error as DeviceError {
            XCTAssertEqual(error, .readFailed)                     // S3
        }
        let retry = try await transport.listRoutes()
        XCTAssertFalse(retry.isEmpty)                              // retry recovers
    }

    func testOutOfRangeShowsBannerButStillServesCachedContent() async throws {
        let (transport, control) = transport(.outOfRange)
        XCTAssertEqual(control.connection, .outOfRange)           // S4 banner
        let cached = try await transport.listRoutes()
        XCTAssertFalse(cached.isEmpty)                            // content still renders
    }

    func testNoDeviceBlocksReads() async {
        let (transport, control) = transport(.noDevice)
        XCTAssertEqual(control.connection, .disconnected)        // A / D1
        do {
            _ = try await transport.listRoutes()
            XCTFail("expected notConnected")
        } catch let error as DeviceError {
            XCTAssertEqual(error, .notConnected)                 // H4 on import
        } catch {
            XCTFail("unexpected error \(error)")
        }
    }

    func testPairingFailures() async {
        // #297: `.timeout` fails in the un-gated scan (`discover`), `.rejected` in
        // the gated `authenticate` — `connect()` runs both, so both surface here.
        await assertConnectThrows(.pairingTimeout, .deviceNotFound)   // D5
        await assertConnectThrows(.pairingRejected, .pairingFailed)   // D5
    }

    func testRadioStates() async {
        await assertConnectThrows(.bluetoothOff, .bluetoothUnavailable(.poweredOff))       // H8
        await assertConnectThrows(.permissionDenied, .bluetoothUnavailable(.unauthorized)) // H7
    }

    func testSyncDropArmsATransferDrop() {
        XCTAssertNotNil(Scenario.syncDrop.preset.dropAtFraction)     // H10
        XCTAssertNotNil(Scenario.uploadDrop.preset.dropAtFraction)   // F interrupted
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
