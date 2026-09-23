#if DEBUG
import Foundation
import OBCDomain
import OBCTransport

public enum FirmwareDemoStage: String, Sendable, Equatable {
    /// Pre-stage a sample update and stop.
    case staged
    /// Also fire Send, so a run walks transferring → awaiting-confirm → done.
    case sending = "send"
}

public struct MockLaunchOptions: Equatable, Sendable {
    public var scenario: Scenario?
    public var fixtures: String?
    public var connection: ConnectionState?
    /// Force the real `BLETransport`. Device only: the simulator has no BLE.
    public var useBLETransport: Bool
    /// Present the dev control panel immediately at launch.
    public var showDevPanel: Bool
    /// Present the OBCUI component gallery immediately at launch.
    public var showUIGallery: Bool
    /// Suppress the Debug scenario tag for product screenshots. The mock transport stays active.
    public var hideMockHUD: Bool
    /// Run the UI with animations off, so an automated capture cannot catch a transition
    /// mid-flight. It affects presentation only, never what is finally drawn.
    public var disableAnimations: Bool
    /// Hold every timed confirmation instead of letting it expire. Each one counts down
    /// against a wall clock, which a loaded CI runner cannot photograph reliably.
    public var holdConfirmations: Bool
    /// Feed a `SampleRouteFile` to the import path at launch: automation cannot drive the
    /// Files picker. `bad` raises the import error. `nil` means no import.
    public var importSample: SampleRouteFile.Kind?
    /// Pin the MapKit-basemap reachability: `false` forces the grid fallback, `true` the
    /// basemap. `nil` uses the real `NWPathMonitor`.
    public var networkOnline: Bool?
    /// Open the firmware-update screen at launch with a pre-staged sample update, because
    /// automation cannot drive the Files picker.
    public var firmwareDemo: FirmwareDemoStage?
    /// Pad the mock device's route catalog to the 64-route resident-menu boundary.
    public var deviceRoutesFull: Bool

    public var oldFirmware: Bool
    /// The simulator photo library's access state. `nil` starts undetermined, as a fresh install.
    public var photoAccess: PhotoAccess?
    /// The mock library's last photo is gone after it is added.
    public var photoGone: Bool
    /// Every request to the mock router fails this way. `nil` routes.
    public var routerFailure: LegRouteFailure?

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
        oldFirmware: Bool = false,
        photoAccess: PhotoAccess? = nil,
        photoGone: Bool = false,
        routerFailure: LegRouteFailure? = nil
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
        self.photoAccess = photoAccess
        self.photoGone = photoGone
        self.routerFailure = routerFailure
    }

    /// Parse process launch arguments (`-OBCKey value` pairs, flag args) with environment
    /// fallbacks. An unknown value degrades to the default: an automation typo must not crash.
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
        // Bare `-OBCImportSample` (or `OBC_IMPORT_SAMPLE=1`) means gpx; an unknown kind
        // degrades to gpx.
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
        // Bare `-OBCFirmwareDemo` (or `OBC_FIRMWARE_DEMO=1`) stops at the staged screen.
        // A `send` token also fires Send; an unknown token stops at staged.
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
        let photoAccess: PhotoAccess? = switch value("OBCPhotoAccess", env: "OBC_PHOTO_ACCESS") {
        case "full": .full
        case "limited": .limited
        case "denied": .denied
        default: nil
        }
        let photoGone = arguments.contains("-OBCPhotoGone")
            || environment["OBC_PHOTO_GONE"] == "1"
        let routerFailure: LegRouteFailure? = switch value("OBCRouter", env: "OBC_ROUTER") {
        case "noRoad": .noRoad
        case "offline": .noConnection
        case "mapData": .mapData
        default: nil
        }

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
            oldFirmware: oldFirmware,
            photoAccess: photoAccess,
            photoGone: photoGone,
            routerFailure: routerFailure)
    }

    public func makeControl() -> MockControl {
        let control = MockControl(scenario: scenario ?? .happyPath)
        if let fixtures { control.loadFixtures(fixtures) }
        if let connection { control.connection = connection }
        control.routesNearlyFull = deviceRoutesFull
        // The flag forces old-firmware even over a scenario that supports clock sync;
        // it never re-enables it (a `.oldFirmware` scenario stays old).
        if oldFirmware { control.supportsClockSync = false }

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
