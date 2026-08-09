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
/// | `-OBCFixtures <name>` | `default` / `empty` / `large` / `trips` / `website` | override the fixture set (`website` is generated from the landing-page GPX) |
/// | `-OBCConnection <state>` | `disconnected` / `connecting` / `connected` / `outOfRange` | override the initial link state |
/// | `-OBCTransport <kind>` | `ble` / `mock` | force the real `BLETransport` in a Debug build |
/// | `-OBCShowDevPanel` | (flag) | present the dev control panel at launch |
/// | `-OBCShowUIGallery` | (flag) | present the B11 component gallery at launch |
/// | `-OBCHideMockHUD` | (flag) | hide the Debug scenario HUD for clean automated captures |
/// | `-OBCDisableAnimations` | (flag) | run the UI without animations so an automated capture can't catch a transition mid-flight (#1212) |
/// | `-OBCHoldConfirmations` | (flag) | park every timed confirmation state instead of letting it expire, so a capture of one isn't a race against a wall clock (#1212). Three holds, all stretched to an hour: the 2 s top-bar sync check, the 60 s "Synced N new rides just now" line (both `RideSyncCoordinator.Timing`), and the upload sheet's 2.6 s self-dismiss (`UploadSheetModel.Timing`) |
/// | `-OBCImportSample [kind]` | bare flag = `gpx`; or `gpx` / `tcx` / `bad` / `grimsel` | feed a bundled sample file to the import path at launch (E1; `bad` → H5; `grimsel` = generated website route) |
/// | `-OBCNetwork <state>` | `offline` / `online` | pin the MapKit-basemap reachability (#294) — `offline` forces the grid fallback |
/// | `-OBCFirmwareDemo` | (flag) | open the S7 firmware-update screen with a pre-staged sample update (the Files picker can't be automated) |
/// | `-OBCDeviceRoutesFull` | (flag) | pad the device's route catalog to one below the cap so a multi-stage trip fails the whole-trip precheck (TR8 storage-precheck demo/test) |
/// | `-OBCOldFirmware` | (flag) | model a device predating auto-expiry (epic #638): `setClock`/`setRouteRetention` answer `unsupported`, no `routeList` expiry tail — S7's capability-gated (hidden) state |
///
/// Env fallbacks (used when the argument is absent): `OBC_SCENARIO`,
/// `OBC_FIXTURES`, `OBC_CONNECTION`, `OBC_TRANSPORT`, `OBC_SHOW_DEV_PANEL=1`,
/// `OBC_SHOW_UI_GALLERY=1`, `OBC_HIDE_MOCK_HUD=1`, `OBC_DISABLE_ANIMATIONS=1`,
/// `OBC_HOLD_CONFIRMATIONS=1`, `OBC_IMPORT_SAMPLE=1` (or a kind token),
/// `OBC_NETWORK`, `OBC_FIRMWARE_DEMO=1`.
/// How far the `-OBCFirmwareDemo` hook drives the S7 screen. Raw values are the
/// launch tokens (`-OBCFirmwareDemo` bare = `staged`, `-OBCFirmwareDemo send`).
public enum FirmwareDemoStage: String, Sendable, Equatable {
    /// Pre-stage a sample update and stop — the "staged" screenshot.
    case staged
    /// Also fire Send, so a run walks transferring → awaiting-confirm → done.
    case sending = "send"
}

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
    /// Suppress the bottom-right Debug scenario tag for product screenshots. The mock transport
    /// remains active; ordinary XCUITests keep the HUD unless they opt out explicitly.
    public var hideMockHUD: Bool
    /// Run the UI with animations off (#1212) — screen pushes, sheet
    /// presentations, and list insertions land in their final state on the frame
    /// they happen. Automated captures need this: a screenshot taken the instant
    /// an element exists must not catch a transition mid-flight. Affects
    /// presentation only, never what is finally drawn.
    public var disableAnimations: Bool
    /// Hold every timed confirmation instead of letting it expire (#1212). Three
    /// of them are real product beats and all three are impossible to photograph
    /// reliably, because each counts down against a wall clock while a loaded CI
    /// runner takes its time: the top-bar sync check (2 s), the "Synced N new
    /// rides just now" line under it (60 s), and the upload sheet's self-dismiss
    /// after "On the device" (2.6 s). Under this flag the composition root hands
    /// each owner a hold long enough that the state simply parks.
    public var holdConfirmations: Bool
    /// Feed a `SampleRouteFile` to the import path at launch (XCUITests /
    /// demos — the Files picker can't be driven from automation): `gpx`/`tcx`/`grimsel`
    /// land on E1 (or H4 when unpaired), `bad` raises H5. `nil` = no import.
    public var importSample: SampleRouteFile.Kind?
    /// Force the MapKit-basemap reachability (#294): `false` pins the grid
    /// fallback (offline), `true` pins the basemap; `nil` uses the real
    /// `NWPathMonitor`. Lets XCUITests exercise the fallback without real network
    /// flakiness.
    public var networkOnline: Bool?
    /// Open the S7 firmware-update screen at launch with a pre-staged sample
    /// update (the Files picker can't be driven from automation) — for the flow
    /// screenshots + demos. `.staged` stops at the staged screen; `.sending`
    /// also fires Send, so a run walks transfer → confirm → done. Debug-only.
    public var firmwareDemo: FirmwareDemoStage?
    /// Pad the mock device's route catalog to one below the route cap so a
    /// multi-stage trip fails the whole-trip precheck **before any bytes** (TR8,
    /// issue #657) — the storage-precheck-failure XCUITest / demo hook.
    public var deviceRoutesFull: Bool
    /// Model a device that predates auto-expiry (epic #638): `setClock` /
    /// `setRouteRetention` answer `unsupported` and `routeList` entries carry no
    /// expiry tail. `false` = the current firmware (expiry supported). Drives
    /// S7's capability-gated (hidden) state under automation.
    public var oldFirmware: Bool

    public init(
        scenario: Scenario? = nil,
        fixtures: String? = nil,
        connection: ConnectionState? = nil,
        useBLETransport: Bool = false,
        showDevPanel: Bool = false,
        showUIGallery: Bool = false,
        hideMockHUD: Bool = false,
        disableAnimations: Bool = false,
        holdConfirmations: Bool = false,
        importSample: SampleRouteFile.Kind? = nil,
        networkOnline: Bool? = nil,
        firmwareDemo: FirmwareDemoStage? = nil,
        deviceRoutesFull: Bool = false,
        oldFirmware: Bool = false
    ) {
        self.scenario = scenario
        self.fixtures = fixtures
        self.connection = connection
        self.useBLETransport = useBLETransport
        self.showDevPanel = showDevPanel
        self.showUIGallery = showUIGallery
        self.hideMockHUD = hideMockHUD
        self.disableAnimations = disableAnimations
        self.holdConfirmations = holdConfirmations
        self.importSample = importSample
        self.networkOnline = networkOnline
        self.firmwareDemo = firmwareDemo
        self.deviceRoutesFull = deviceRoutesFull
        self.oldFirmware = oldFirmware
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
        let hideMockHUD = arguments.contains("-OBCHideMockHUD")
            || environment["OBC_HIDE_MOCK_HUD"] == "1"
        let disableAnimations = arguments.contains("-OBCDisableAnimations")
            || environment["OBC_DISABLE_ANIMATIONS"] == "1"
        let holdConfirmations = arguments.contains("-OBCHoldConfirmations")
            || environment["OBC_HOLD_CONFIRMATIONS"] == "1"
        // Flag with an optional kind token: bare `-OBCImportSample` (or
        // `OBC_IMPORT_SAMPLE=1`) means gpx; an unknown kind degrades to gpx
        // (never crash — automation typo rule).
        let importSample: SampleRouteFile.Kind? = {
            if let index = arguments.firstIndex(of: "-OBCImportSample") {
                if index + 1 < arguments.count, !arguments[index + 1].hasPrefix("-") {
                    return SampleRouteFile.Kind(rawValue: arguments[index + 1]) ?? .gpx
                }
                return .gpx
            }
            guard let env = environment["OBC_IMPORT_SAMPLE"], !env.isEmpty, env != "0" else {
                return nil
            }
            return SampleRouteFile.Kind(rawValue: env) ?? .gpx
        }()
        // Unknown tokens leave reachability on the real monitor (nil).
        let networkOnline: Bool? = switch value("OBCNetwork", env: "OBC_NETWORK") {
        case "offline": false
        case "online": true
        default: nil
        }
        // Bare `-OBCFirmwareDemo` (or `OBC_FIRMWARE_DEMO=1`) stops at the staged
        // screen; a `send` token also fires Send (unknown token → staged).
        let firmwareDemo: FirmwareDemoStage? = {
            if let index = arguments.firstIndex(of: "-OBCFirmwareDemo") {
                if index + 1 < arguments.count, !arguments[index + 1].hasPrefix("-") {
                    return FirmwareDemoStage(rawValue: arguments[index + 1]) ?? .staged
                }
                return .staged
            }
            guard let env = environment["OBC_FIRMWARE_DEMO"], !env.isEmpty, env != "0" else {
                return nil
            }
            return FirmwareDemoStage(rawValue: env) ?? .staged
        }()

        let deviceRoutesFull = arguments.contains("-OBCDeviceRoutesFull")
            || environment["OBC_DEVICE_ROUTES_FULL"] == "1"
        let oldFirmware = arguments.contains("-OBCOldFirmware")
            || environment["OBC_OLD_FIRMWARE"] == "1"

        return MockLaunchOptions(
            scenario: scenario,
            fixtures: fixtures,
            connection: connection,
            useBLETransport: transport == "ble",
            showDevPanel: showPanel,
            showUIGallery: showGallery,
            hideMockHUD: hideMockHUD,
            disableAnimations: disableAnimations,
            holdConfirmations: holdConfirmations,
            importSample: importSample,
            networkOnline: networkOnline,
            firmwareDemo: firmwareDemo,
            deviceRoutesFull: deviceRoutesFull,
            oldFirmware: oldFirmware
        )
    }

    /// Build the live `MockControl` these options describe: scenario preset first,
    /// then the fixture / connection overrides on top.
    public func makeControl() -> MockControl {
        let control = MockControl(scenario: scenario ?? .happyPath)
        if let fixtures { control.loadFixtures(fixtures) }
        if let connection { control.connection = connection }
        control.routesNearlyFull = deviceRoutesFull
        // The flag forces old-firmware even over a scenario that supports expiry;
        // it never re-enables it (a `.oldFirmware` scenario stays old).
        if oldFirmware { control.supportsExpiry = false }
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
