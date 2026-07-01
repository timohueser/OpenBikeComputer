import XCTest
import OBCDomain
import OBCTransport
@testable import OBCMock

/// The B1P launch-arg surface: `-OBCScenario` & friends parse into
/// `MockLaunchOptions` and produce a correctly-configured `MockControl`. These
/// names are stable automation API — a rename here must be deliberate (and
/// update CLAUDE.md + the XCUITest helper).
final class MockLaunchOptionsTests: XCTestCase {
    private func parse(_ args: [String], env: [String: String] = [:]) -> MockLaunchOptions {
        // Real argv always has the executable path first; mirror that.
        MockLaunchOptions.parse(arguments: ["OBCCompanion"] + args, environment: env)
    }

    func testDefaultsAreEmpty() {
        let options = parse([])
        XCTAssertEqual(options, MockLaunchOptions())
    }

    func testParsesEveryScenarioToken() {
        for scenario in Scenario.allCases {
            let options = parse(["-OBCScenario", scenario.rawValue])
            XCTAssertEqual(options.scenario, scenario, "token '\(scenario.rawValue)' must round-trip")
        }
    }

    func testParsesFixturesConnectionTransportAndPanelFlag() {
        let options = parse([
            "-OBCScenario", "outOfRange",
            "-OBCFixtures", "large",
            "-OBCConnection", "connecting",
            "-OBCTransport", "ble",
            "-OBCShowDevPanel",
            "-OBCShowUIGallery",
        ])
        XCTAssertEqual(options.scenario, .outOfRange)
        XCTAssertEqual(options.fixtures, "large")
        XCTAssertEqual(options.connection, .connecting)
        XCTAssertTrue(options.useBLETransport)
        XCTAssertTrue(options.showDevPanel)
        XCTAssertTrue(options.showUIGallery)
    }

    func testEnvironmentFallbacksApplyWhenArgsAbsent() {
        let options = parse([], env: [
            "OBC_SCENARIO": "emptyLibrary",
            "OBC_CONNECTION": "outOfRange",
            "OBC_SHOW_DEV_PANEL": "1",
            "OBC_SHOW_UI_GALLERY": "1",
        ])
        XCTAssertEqual(options.scenario, .emptyLibrary)
        XCTAssertEqual(options.connection, .outOfRange)
        XCTAssertTrue(options.showDevPanel)
        XCTAssertTrue(options.showUIGallery)
        // The argument wins over the environment when both are present.
        let overridden = parse(["-OBCScenario", "readError"], env: ["OBC_SCENARIO": "emptyLibrary"])
        XCTAssertEqual(overridden.scenario, .readError)
    }

    func testUnknownTokensDegradeToDefaultsNotCrash() {
        let options = parse([
            "-OBCScenario", "notAScenario",
            "-OBCConnection", "sideways",
            "-OBCTransport", "carrierPigeon",
        ])
        XCTAssertNil(options.scenario)
        XCTAssertNil(options.connection)
        XCTAssertFalse(options.useBLETransport)
        // A trailing key with no value is ignored too.
        XCTAssertNil(parse(["-OBCScenario"]).scenario)
    }

    func testMakeControlAppliesScenarioThenOverrides() async throws {
        var options = MockLaunchOptions(scenario: .emptyLibrary, connection: .outOfRange)
        var control = options.makeControl()
        XCTAssertEqual(control.scenario, .emptyLibrary)
        XCTAssertTrue(control.fixtures.routes.isEmpty)            // empty fixture set
        XCTAssertEqual(control.connection, .outOfRange)           // override on top

        // Fixture override beats the scenario's own fixture choice.
        options = MockLaunchOptions(scenario: .happyPath, fixtures: "empty")
        control = options.makeControl()
        XCTAssertTrue(control.fixtures.routes.isEmpty)

        // No options at all → plain happyPath.
        control = MockLaunchOptions().makeControl()
        XCTAssertEqual(control.scenario, .happyPath)
        XCTAssertFalse(control.fixtures.routes.isEmpty)
    }
}
