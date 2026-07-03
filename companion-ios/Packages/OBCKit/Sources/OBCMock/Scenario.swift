#if DEBUG
import Foundation
import OBCDomain

/// A named bundle of `MockControl` knobs that reproduces one (or a few) design
/// screens with no device and no firmware. The `rawValue` doubles as the
/// `-OBCScenario` launch-arg token.
///
/// | Scenario | Reproduces |
/// |---|---|
/// | `happyPath` | C1 / C2 / E2 / F / F₂ |
/// | `emptyLibrary` | S1 |
/// | `coldRead` | S2 (skeletons) |
/// | `readError` | S3 |
/// | `outOfRange` | S4 + disconnected banner |
/// | `noDevice` | D1→D4 pairing flow; H4 on import |
/// | `pairingTimeout` / `pairingRejected` | D5 |
/// | `bluetoothOff` / `permissionDenied` | H8 / H7 |
/// | `syncUpToDate` / `syncDrop` | H9 / H10 |
/// | `uploadDrop` | F interrupted → restart |
/// | `unsupportedFile` | H5 |
///
/// Some rows are pure UI-layer states the transport can't originate — `unsupportedFile`
/// is import validation, `syncUpToDate` is "no new rides" — so their preset is a happy
/// link and the UI branches on `scenario`. The rest are fully transport-driven.
public enum Scenario: String, CaseIterable, Sendable {
    case happyPath
    case emptyLibrary
    case coldRead
    case readError
    case outOfRange
    case noDevice
    case pairingTimeout
    case pairingRejected
    case bluetoothOff
    case permissionDenied
    case syncUpToDate
    case syncDrop
    case uploadDrop
    case unsupportedFile
}

/// The concrete knob values a `Scenario` expands to. Public so the dev panel /
/// tests can inspect or compose presets.
public struct ScenarioPreset: Sendable {
    /// Bundled fixture-set name to load.
    public var fixtures: String
    /// Initial connection state the `state` stream replays.
    public var connection: ConnectionState
    /// Whether the app has bonded before — the launch branch (`MockBondStore`
    /// reads it). False for the pairing-flow scenarios (they start unpaired);
    /// true everywhere else.
    public var bonded: Bool
    /// Radio power/permission.
    public var radio: RadioState
    /// Per-op latency.
    public var latency: Duration
    /// Bulk-transfer throughput (bytes/sec).
    public var throughputBytesPerSec: Int
    /// A one-shot failure armed on the next throwing op (nil = none).
    public var pendingFailure: DeviceError?
    /// A pairing failure armed on the next `connect()` (nil = none).
    public var pairingFail: PairingFail?
    /// A drop point armed on the next transfer, as a fraction 0…1 (nil = none).
    public var dropAtFraction: Double?

    public init(
        fixtures: String = "default",
        connection: ConnectionState = .connected,
        bonded: Bool = true,
        radio: RadioState = .on,
        latency: Duration = .milliseconds(180),
        throughputBytesPerSec: Int = 500_000,
        pendingFailure: DeviceError? = nil,
        pairingFail: PairingFail? = nil,
        dropAtFraction: Double? = nil
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
    }
}

extension Scenario {
    /// The knob bundle this scenario expands to (see the table on `Scenario`).
    public var preset: ScenarioPreset {
        switch self {
        case .happyPath:
            return ScenarioPreset()
        case .emptyLibrary:
            return ScenarioPreset(fixtures: "empty")
        case .coldRead:
            // A slow first read → the UI shows S2 skeletons while it awaits.
            return ScenarioPreset(latency: .seconds(3))
        case .readError:
            // The first read throws → S3; a retry (next read) succeeds.
            return ScenarioPreset(pendingFailure: .readFailed)
        case .outOfRange:
            return ScenarioPreset(connection: .outOfRange)
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
        }
    }
}
#endif
