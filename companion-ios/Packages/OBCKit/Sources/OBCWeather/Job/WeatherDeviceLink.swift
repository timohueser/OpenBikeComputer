import Foundation
import OBCDomain

/// One device weather request as WX9 checkpoints it — the durable, transport-free image of a
/// `weatherRequestContext` read (spec §11.4).
///
/// This is deliberately `Codable`: the epic's two-connection shape means the app may be suspended
/// or killed between the context read and the upload, so the read's result must outlive the
/// process. It is persisted by ``WeatherJobStore`` the moment the read completes — *before* any
/// network work — which is what lets a relaunched job resume from here instead of spending a second
/// connection re-reading what it already knows.
///
/// Privacy note: the rider coordinate in here exists so the fetch phase can run after a relaunch.
/// It lives only in the app's private job checkpoint and is deleted with the job; it must never be
/// copied into ``WeatherJobHistoryEntry`` (the WX13 diagnostics ring), which is coordinate-free by
/// construction.
public struct WeatherDeviceRequestSnapshot: Codable, Equatable, Sendable {
    /// The device's request nonce (§11.2) — stable across the device's retry ladder, echoed into
    /// the OBCW header so the two connections correlate.
    public var requestID: UInt32
    /// Where the rider was, when the device vouched for a fix. Optional as a *group*, mirroring the
    /// wire's validity bit — never latitude 0.
    public var latitudeMicrodegrees: Int32?
    public var longitudeMicrodegrees: Int32?
    public var fixUnixSeconds: Int64?
    public var bearingDegrees: Double?
    public var speedMetresPerSecond: Double?
    public var routeID: UInt16?
    /// The bundle the device already holds, when it holds one — what generation arithmetic and the
    /// staleness check key on.
    public var heldBundleGeneration: UInt32?
    public var heldBundleGeneratedAtUnixSeconds: Int64?
    /// The raw §11.4 reason word — advisory scheduling help, carried for diagnostics; never a gate.
    public var reasonRawValue: UInt16
    /// When the context read completed (phone clock).
    public var readAt: Date

    public init(
        requestID: UInt32,
        latitudeMicrodegrees: Int32? = nil,
        longitudeMicrodegrees: Int32? = nil,
        fixUnixSeconds: Int64? = nil,
        bearingDegrees: Double? = nil,
        speedMetresPerSecond: Double? = nil,
        routeID: UInt16? = nil,
        heldBundleGeneration: UInt32? = nil,
        heldBundleGeneratedAtUnixSeconds: Int64? = nil,
        reasonRawValue: UInt16 = 0,
        readAt: Date
    ) {
        self.requestID = requestID
        self.latitudeMicrodegrees = latitudeMicrodegrees
        self.longitudeMicrodegrees = longitudeMicrodegrees
        self.fixUnixSeconds = fixUnixSeconds
        self.bearingDegrees = bearingDegrees
        self.speedMetresPerSecond = speedMetresPerSecond
        self.routeID = routeID
        self.heldBundleGeneration = heldBundleGeneration
        self.heldBundleGeneratedAtUnixSeconds = heldBundleGeneratedAtUnixSeconds
        self.reasonRawValue = reasonRawValue
        self.readAt = readAt
    }

    /// The assembler's input, mapped from the wire groups. A cleared position group becomes
    /// `position == nil` here — the job then fails as ``WeatherJobFailure/noPosition`` rather than
    /// fetching for the Gulf of Guinea.
    public var weatherRequest: WeatherRequest {
        var position: Coordinate?
        if let latitudeMicrodegrees, let longitudeMicrodegrees {
            position = Coordinate(
                latitude: Double(latitudeMicrodegrees) / 1_000_000,
                longitude: Double(longitudeMicrodegrees) / 1_000_000)
        }
        return WeatherRequest(
            requestID: requestID,
            position: position,
            fixTime: fixUnixSeconds.map { Date(timeIntervalSince1970: TimeInterval($0)) },
            bearingDegrees: bearingDegrees,
            speedMetresPerSecond: speedMetresPerSecond)
    }

    /// The generation the next built bundle must carry: serially one past whatever the device
    /// holds (§11.6's RFC-1982 comparison makes `&+ 1` correct across the wrap), or `1` for a
    /// device that holds nothing.
    public var nextGeneration: UInt32 {
        (heldBundleGeneration ?? 0) &+ 1
    }
}

/// Why one leg of the device conversation failed, in the job engine's vocabulary.
///
/// The distinction that matters to the engine is **retryable versus reproducible**: a dropped link
/// or a timeout is the transport being a radio, and the device's own 5/10/20-minute ladder will
/// re-raise the request; `bundleRejected` is the device saying *these bytes arrived intact and are
/// not a bundle* (spec §11.5's `error`), where re-sending the same bytes reproduces the failure and
/// the fix is a rebuild — or a producer bug to surface, never an infinite retry.
public enum WeatherDeviceLinkError: Error, Equatable, Sendable {
    /// No bonded device is known — there is nothing to talk to and no ladder coming.
    case noBondedDevice
    case bluetoothUnavailable
    case timedOut
    case connectionDropped
    /// The context read arrived but could not be decoded.
    case malformedContext
    /// The wire corrupted the upload (`crcMismatch`) — a retry re-sends the same, correct bytes.
    case transferCorrupted
    /// The device validated our CRC and refused the content (§11.5 `error`) — reproducible.
    case bundleRejected
    /// The device answered `busy` / another exchange holds the link — retry later.
    case deviceBusy
    /// The transport is already running a weather exchange for another caller.
    case linkBusy
    case cancelled
}

/// One completed context read: the snapshot plus its timing evidence.
public struct WeatherContextReadReceipt: Equatable, Sendable {
    public var snapshot: WeatherDeviceRequestSnapshot
    /// How long the radio was held for the read leg.
    public var connectedDuration: Duration
    public var reusedForegroundConnection: Bool

    public init(
        snapshot: WeatherDeviceRequestSnapshot, connectedDuration: Duration,
        reusedForegroundConnection: Bool
    ) {
        self.snapshot = snapshot
        self.connectedDuration = connectedDuration
        self.reusedForegroundConnection = reusedForegroundConnection
    }
}

/// Evidence from one completed upload leg — feeds the WX13 ring so the epic's connected-time
/// targets (≤ 5 s median / ≤ 10 s p95) are measurable from job telemetry.
public struct WeatherBundleUploadReceipt: Equatable, Sendable {
    /// Time from asking for the link to the connection coming up (zero when one was reused).
    public var connectLatency: Duration
    /// Time the radio was held: gated phase + descriptor + payload + verdict.
    public var connectedDuration: Duration
    /// True when the upload rode a user-owned foreground session (which the job must never tear
    /// down) instead of its own ephemeral connection.
    public var reusedForegroundConnection: Bool

    public init(
        connectLatency: Duration, connectedDuration: Duration, reusedForegroundConnection: Bool
    ) {
        self.connectLatency = connectLatency
        self.connectedDuration = connectedDuration
        self.reusedForegroundConnection = reusedForegroundConnection
    }
}

/// The transport seam the job engine drives — both short connections of the §11 exchange, and
/// nothing else. `BLETransport` conforms via `WeatherBLEDeviceLink` (OBCTransport); tests conform
/// with scripted mocks, which is what makes the whole job host-testable with no CoreBluetooth in
/// the process.
///
/// Both calls are **one-shot and bounded** by contract: they acquire the link, do one thing, and
/// let go. Neither may hold BLE across network work — the engine's phase order is what guarantees
/// the radio is idle throughout provider HTTP.
public protocol WeatherDeviceLink: Sendable {
    /// Connect (or ride an existing foreground session), read one authenticated
    /// `weatherRequestContext`, disconnect. The advertised request is consumed by this read
    /// (§11.3); the returned snapshot is the job's checkpoint.
    func readRequestContext() async throws -> WeatherContextReadReceipt
    /// Reconnect to the known peripheral (or ride a foreground session), upload one OBCW bundle as
    /// object type 20 / id 0 over the CoC, await the device's `transferResult`, disconnect.
    /// Returns on `committed` — which per §11.6 includes the duplicate/stale ignored-but-successful
    /// rows, each of which finishes the request.
    func uploadBundle(_ bytes: Data) async throws -> WeatherBundleUploadReceipt
}
