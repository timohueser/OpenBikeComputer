import Foundation

// The upload half of the Weather Request exchange (spec §11.5, WX9 / #1194) — pure result/error
// vocabulary, no CoreBluetooth, so it stays host-testable beside `WeatherRequest.swift`.

/// One completed weather-bundle upload: the device answered `committed`, which per §11.6 includes
/// the duplicate/stale ignored-but-successful rows — each is the phone's complete answer and
/// finishes the request.
public struct WeatherBundleUpload: Equatable, Sendable {
    /// From asking for the link to the connection coming up. Zero-ish when an existing foreground
    /// session was reused.
    public var connectLatency: Duration
    /// How long the radio was held for this leg: gated phase + descriptor + payload + verdict.
    public var connectedDuration: Duration
    /// True when the upload rode a user-owned foreground session (never torn down by this op)
    /// instead of its own ephemeral weather connection.
    public var reusedForegroundConnection: Bool

    public init(
        connectLatency: Duration, connectedDuration: Duration, reusedForegroundConnection: Bool
    ) {
        self.connectLatency = connectLatency
        self.connectedDuration = connectedDuration
        self.reusedForegroundConnection = reusedForegroundConnection
    }
}

/// Why one weather-bundle upload attempt did not commit.
///
/// The split that matters to the caller (the WX9 job engine) is *retry the same bytes* versus
/// *rebuild*: everything here except ``rejected`` is link-class — the persisted bundle stays valid
/// and re-uploading it is safe because a duplicate answers `committed` (§11.6). `rejected` is the
/// device saying the bytes arrived intact and are not a bundle (§11.5's `error`), where a retry
/// reproduces the failure.
public enum WeatherUploadError: Error, Equatable, Sendable {
    /// The caller handed an empty payload — a caller bug, failed loudly.
    case emptyPayload
    /// No peripheral has ever completed an authenticated session; there is nothing to connect to.
    case noKnownBondedPeripheral
    case bluetoothUnavailable
    /// A weather upload is already in flight for another caller.
    case busy
    /// The overall or connected budget expired. The attempt may still have committed on the
    /// device after the phone gave up — the §11.6 duplicate row makes the retry harmless.
    case timedOut
    case connectionDropped
    /// The wire corrupted the bytes (`crcMismatch`) — resend the same bytes.
    case crcMismatch
    /// The device answered `busy` — another exchange holds its transfer machinery. Also reported
    /// when a budget expired while this exchange was queued behind another transfer: the link was
    /// busy with someone else's bytes, which is the device/foreground being busy, not this leg
    /// running long.
    case deviceBusy
    /// The device has no room for the bundle right now (`storageFull`). Says nothing about the
    /// bytes — they stay valid and the retry re-sends them once the device has space.
    case storageFull
    /// The device answered `notFound` for the singleton id `0` (§11.5). Also not a verdict on the
    /// bytes: the object slot was unavailable, so the retry re-sends the same bundle.
    case notFound
    /// CRC passed and the device refused the content (§11.5's `error`) — reproducible; the fix is
    /// the producer's, not a retry.
    case rejected
    case cancelled
}
