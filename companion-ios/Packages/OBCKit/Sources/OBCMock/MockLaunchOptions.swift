#if DEBUG
import Foundation
import OBCDomain

/// The launch-argument / environment surface that boots the app into a chosen
/// mock state (B1P) — what XCUITests and screenshot automation drive. Parsed at
/// the composition root; pure over `[String]` + env so it's host-testable.
///
/// **The launch-arg names are stable API** (documented in companion-ios/CLAUDE.md
/// — automation depends on them; don't rename):
///
/// | argument | values | effect |
/// |---|---|---|
/// | `-OBCScenario <name>` | any `Scenario.rawValue` | boot into that scenario |
/// | `-OBCFixtures <name>` | `default` / `empty` / `large` | override the fixture set |
/// | `-OBCConnection <state>` | `disconnected` / `connecting` / `connected` / `outOfRange` | override the initial link state |
/// | `-OBCTransport <kind>` | `ble` / `mock` | force the real `BLETransport` in a Debug build |
/// | `-OBCShowDevPanel` | (flag) | present the dev control panel at launch |
/// | `-OBCShowUIGallery` | (flag) | present the B11 component gallery at launch |
/// | `-OBCImportSample` | (flag) | boot straight into the E1 import landing with the bundled sample GPX |
///
/// Env fallbacks (used when the argument is absent): `OBC_SCENARIO`,
/// `OBC_FIXTURES`, `OBC_CONNECTION`, `OBC_TRANSPORT`, `OBC_SHOW_DEV_PANEL=1`,
/// `OBC_SHOW_UI_GALLERY=1`, `OBC_IMPORT_SAMPLE=1`.
public struct MockLaunchOptions: Equatable, Sendable {
    public var scenario: Scenario?
    public var fixtures: String?
    public var connection: ConnectionState?
    /// Force the real `BLETransport` (device-only — no BLE in the simulator).
    public var useBLETransport: Bool
    /// Present the dev control panel immediately at launch.
    public var showDevPanel: Bool
    /// Present the OBCUI component gallery immediately at launch (B11
    /// screenshot review).
    public var showUIGallery: Bool
    /// Boot straight into the E1 import landing with `SampleRouteFile` (B4
    /// XCUITests / demos — the Files picker can't be driven from automation).
    public var importSample: Bool

    public init(
        scenario: Scenario? = nil,
        fixtures: String? = nil,
        connection: ConnectionState? = nil,
        useBLETransport: Bool = false,
        showDevPanel: Bool = false,
        showUIGallery: Bool = false,
        importSample: Bool = false
    ) {
        self.scenario = scenario
        self.fixtures = fixtures
        self.connection = connection
        self.useBLETransport = useBLETransport
        self.showDevPanel = showDevPanel
        self.showUIGallery = showUIGallery
        self.importSample = importSample
    }

    /// Parse process launch arguments (`-OBCKey value` pairs, flag args) with
    /// environment fallbacks. Unknown values are ignored (the app must still
    /// boot — a typo in an automation script degrades to defaults, not a crash).
    public static func parse(
        arguments: [String] = ProcessInfo.processInfo.arguments,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> MockLaunchOptions {
        func value(_ key: String, env envKey: String) -> String? {
            if let index = arguments.firstIndex(of: "-\(key)"), index + 1 < arguments.count {
                return arguments[index + 1]
            }
            return environment[envKey]
        }

        let scenario = value("OBCScenario", env: "OBC_SCENARIO").flatMap(Scenario.init(rawValue:))
        let fixtures = value("OBCFixtures", env: "OBC_FIXTURES")
        let connection = value("OBCConnection", env: "OBC_CONNECTION").flatMap(ConnectionState.init(launchToken:))
        let transport = value("OBCTransport", env: "OBC_TRANSPORT")
        let showPanel = arguments.contains("-OBCShowDevPanel")
            || environment["OBC_SHOW_DEV_PANEL"] == "1"
        let showGallery = arguments.contains("-OBCShowUIGallery")
            || environment["OBC_SHOW_UI_GALLERY"] == "1"
        let importSample = arguments.contains("-OBCImportSample")
            || environment["OBC_IMPORT_SAMPLE"] == "1"

        return MockLaunchOptions(
            scenario: scenario,
            fixtures: fixtures,
            connection: connection,
            useBLETransport: transport == "ble",
            showDevPanel: showPanel,
            showUIGallery: showGallery,
            importSample: importSample
        )
    }

    /// Build the live `MockControl` these options describe: scenario preset first,
    /// then the fixture / connection overrides on top.
    public func makeControl() -> MockControl {
        let control = MockControl(scenario: scenario ?? .happyPath)
        if let fixtures { control.loadFixtures(fixtures) }
        if let connection { control.connection = connection }
        return control
    }
}

extension ConnectionState {
    /// The launch-arg token for `-OBCConnection` (mirrors the case names).
    public init?(launchToken: String) {
        switch launchToken {
        case "disconnected": self = .disconnected
        case "connecting": self = .connecting
        case "connected": self = .connected
        case "outOfRange": self = .outOfRange
        default: return nil
        }
    }
}
#endif
