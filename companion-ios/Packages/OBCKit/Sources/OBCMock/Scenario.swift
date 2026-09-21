#if DEBUG
import Foundation
import OBCDomain

/// A named bundle of `MockControl` knobs that reproduces design screens with no device and
/// no firmware. The `rawValue` is also the launch-arg token (`-OBCScenario happyPath`).
///
/// Some scenarios are UI-layer states the transport cannot originate: `unsupportedFile` is
/// import validation and `syncUpToDate` means "no new rides". Their preset is a happy link and
/// the UI branches on `scenario`. The rest are transport-driven.
public enum Scenario: String, CaseIterable, Sendable {
    case happyPath
    case emptyLibrary
    case coldRead
    case readError
    case outOfRange
    case deviceUnreachable
    case noDevice
    case pairingTimeout
    case pairingRejected
    case bluetoothOff
    case permissionDenied
    case syncUpToDate
    case syncDrop
    case uploadDrop
    case unsupportedFile
    case oldFirmware
}

/// The concrete knob values a `Scenario` expands to.
public struct ScenarioPreset: Sendable {
    /// Bundled fixture-set name to load.
    public var fixtures: String
    /// Initial connection state the `state` stream replays.
    public var connection: ConnectionState
    /// Whether the app has bonded before. False for the pairing-flow scenarios, true elsewhere.
    public var bonded: Bool
    public var radio: RadioState
    public var latency: Duration
    public var throughputBytesPerSec: Int
    /// A one-shot failure armed on the next throwing op (nil = none).
    public var pendingFailure: DeviceError?
    /// A pairing failure armed on the next `connect()` (nil = none).
    public var pairingFail: PairingFail?
    /// A drop point armed on the next transfer, as a fraction 0…1 (nil = none).
    public var dropAtFraction: Double?
    public var supportsClockSync: Bool

    public init(
        fixtures: String = "default",
        connection: ConnectionState = .connected,
        bonded: Bool = true,
        radio: RadioState = .on,
        latency: Duration = .milliseconds(180),
        throughputBytesPerSec: Int = 500_000,
        pendingFailure: DeviceError? = nil,
        pairingFail: PairingFail? = nil,
        dropAtFraction: Double? = nil,
        supportsClockSync: Bool = true
    ) {
        self.fixtures = fixtures
        self.connection = connection
        self.bonded = bonded
        self.radio = radio
        self.latency = latency
        self.throughputBytesPerSec = throughputBytesPerSec
        self.pendingFailure = pendingFailure
        self.pairingFail = pairingFail
        self.dropAtFraction = dropAtFraction
        self.supportsClockSync = supportsClockSync
    }
}

extension Scenario {
    public var preset: ScenarioPreset {
        switch self {
        case .happyPath:
            return ScenarioPreset()
        case .emptyLibrary:
            return ScenarioPreset(fixtures: "empty")
        case .coldRead:
            // A slow first read: the UI shows skeletons while it awaits.
            return ScenarioPreset(latency: .seconds(3))
        case .readError:
            // The first read throws; the next read succeeds.
            return ScenarioPreset(pendingFailure: .readFailed)
        case .outOfRange:
            return ScenarioPreset(connection: .outOfRange)
        case .deviceUnreachable:
            // Bonded but the device never answers: connect() parks on the huge latency the
            // way a real scan parks on an absent peripheral. Launch must time out, not hang.
            return ScenarioPreset(connection: .disconnected, latency: .seconds(3_600))
        case .noDevice:
            return ScenarioPreset(connection: .disconnected, bonded: false)
        case .pairingTimeout:
            return ScenarioPreset(connection: .disconnected, bonded: false, pairingFail: .timeout)
        case .pairingRejected:
            return ScenarioPreset(connection: .disconnected, bonded: false, pairingFail: .rejected)
        case .bluetoothOff:
            return ScenarioPreset(connection: .disconnected, bonded: false, radio: .off)
        case .permissionDenied:
            return ScenarioPreset(connection: .disconnected, bonded: false, radio: .unauthorized)
        case .syncUpToDate:
            return ScenarioPreset()
        case .syncDrop:
            return ScenarioPreset(dropAtFraction: 0.42)
        case .uploadDrop:
            return ScenarioPreset(dropAtFraction: 0.62)
        case .unsupportedFile:
            return ScenarioPreset()
        case .oldFirmware:
            // A supported peer without clock sync; a happy link otherwise.
            return ScenarioPreset(supportsClockSync: false)
        }
    }
}
#endif
