import Foundation
import OBCDomain

/// The device's answer to a `setClock` write (spec §4.4 cmd 5, epic #638). Not an
/// error type — a device that predates expiry (`unsupported`) is a **supported
/// peer**; the app degrades gracefully (hides expiry UI, sends no retention).
public enum ClockSyncOutcome: Equatable, Sendable {
    /// The device stamped its trusted clock — it understands expiry (`commandResult(ok)`).
    case stamped
    /// The device answered `unknownCommand` — it predates expiry support. S7 hides
    /// the expiry UI behind this and the app sends no `setRouteRetention`.
    case unsupported
}

/// The device's answer to a `setRouteRetention` write (spec §4.4 cmd 6, epic #638).
public enum RetentionWriteOutcome: Equatable, Sendable {
    /// The level was written (`commandResult(ok)`) — the device bumps its route
    /// store revision only on a real change (idempotent re-set is also `ok`).
    case applied
    /// The device holds no route under that id (`commandResult(notFound)`) — the
    /// id raced a device-side delete; reconcile clears the stale link.
    case notFound
    /// The device answered `unknownCommand` — it predates expiry support.
    case unsupported
}

/// The device link lifecycle and identity handshake, without unrelated capabilities.
public protocol DeviceLink: Sendable {
    /// Link lifecycle. **Replays the latest** value to late subscribers (a fresh
    /// stream immediately yields the current state), then streams changes.
    var state: AsyncStream<ConnectionState> { get }
    /// Begin connecting: power-on wait, scan, connect, discover, open the CoC.
    /// Throws `DeviceError` on failure (never traps). The full link = `discover()`
    /// then `authenticate()`. The protocol-version check (#303) runs where
    /// `deviceInfo()` is consumed on connect — a mismatch surfaces as a banner +
    /// disabled sync, not a thrown connect (which would mis-degrade to S4).
    func connect() async throws
    /// **First-time-pairing phase 1** (#297): power-on wait, scan, connect, and
    /// discover services + only the **un-gated** characteristics (DIS / BAS /
    /// `protocolVersion`) — enough for `deviceInfo()` and the D2 device row, but
    /// touching **no** gated characteristic, so iOS does *not* raise the LESC
    /// passkey sheet yet. The gated ops wait for `authenticate()`.
    func discover() async throws
    /// **First-time-pairing phase 2** (#297): the gated operations that establish
    /// the encrypted, LESC-authenticated link — subscribe the `status` notify
    /// (the sole device → app CCCD in v2), read the PSM, open the CoC. This is what raises
    /// the system passkey sheet (A8); the launch flow calls it on the D2 row tap so
    /// the sheet lands in the D3 "pairing…" beat. Requires a prior `discover()`.
    func authenticate() async throws
    /// Tear the link down.
    func disconnect() async
    /// Foreground-only lifecycle, background half (#459): drop the link **and
    /// pause the transport's own reconnect behaviour** until `resumeLink()`.
    /// The app calls this on a real `scenePhase == .background` transition (after
    /// any in-flight transfer drained) — a transport that kept scanning or held
    /// a pending connect would fight the intentional disconnect and re-raise
    /// the link behind the user's back.
    func suspendLink() async
    /// Foreground-only lifecycle, foreground half (#459): undo `suspendLink()`
    /// by re-arming the reconnect machinery — the **existing bonded
    /// silent-reconnect path**, never a fresh pairing flow. Failure is silent
    /// (the S4 banner owns the degraded-link story); callers only invoke this
    /// when a link existed before the suspend.
    func resumeLink() async
    /// Device identity (DIS + `protocol_version`) established by discovery.
    func deviceInfo() async throws -> DeviceInfo
}

/// The device configuration control plane, separated from link and object
/// transport so config-only policies do not acquire unrelated capabilities.
public protocol DeviceConfiguration: Sendable {
    /// Read the device config blob.
    func readConfig() async throws -> DeviceConfig
    /// Write the device config blob — including device rename (H3, Delta 1).
    func writeConfig(_ config: DeviceConfig) async throws
}

/// Stored route, trip, and ride operations, without link lifecycle, device
/// configuration, diagnostics, weather discovery, or firmware update authority.
public protocol DeviceObjects: Sendable {
    // MARK: Data plane (bulk objects — progress + cancel + restart)
    //
    // Ids on this plane are **device-namespace** (`DeviceObjectID`, spec §4.1 —
    // durable for the life of the stored object). The types enforce the split
    // (#359): route ops take `DeviceObjectID` directly (a library `RouteID`
    // can't cross this boundary — `PlannedRouteRecord.deviceObjectID` is the
    // app's durable link), and ride ids are minted from the catalog via
    // `RideID(deviceObjectID:)`.

    /// Enumerate routes stored on the device — reconcile input for the
    /// "on device" badge (#289), never Planned-list rows.
    func listRoutes() async throws -> [RouteCatalogEntry]
    /// Full detail for one stored route: the stored OBCR object, decoded
    /// app-side (spec §7.1 — "download the route object").
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail
    /// Upload a route (app → device, B5). Success is the device's committed
    /// `transferResult`; `resume()` after a drop restarts the whole upload.
    func uploadRoute(_ route: RouteBlob) -> TransferHandle
    /// Delete a route from the device.
    func deleteRoute(_ id: DeviceObjectID) async throws
    /// Enumerate trips stored on the device — the `tripList` object (type 10,
    /// spec §7.4) decoded to the reconcile catalog (TR8). Reconcile input for the
    /// trip card's "on device" badge (the per-entry `crc32` is the fingerprint),
    /// never list rows (trips are library-first, like routes).
    func listTrips() async throws -> [TripCatalogEntry]
    /// Full contents of one stored trip object (spec §7.7 — "download the trip
    /// object"): the name + the stage device ids in ride order, dangling refs
    /// included. Reconcile only fetches this when the `tripList` fingerprint can't
    /// decide (the primary check is the entry's `crc32`).
    func downloadTrip(_ id: DeviceObjectID) async throws -> TripObjectCodec.Decoded
    /// Upload a whole trip object (app → device, TR8) — the trip sibling of
    /// `uploadRoute`. A fresh trip sends `0xFFFF` (the device mints an id from its
    /// own trip counter); a re-push / adoption sends the stored id to replace it
    /// in place. Success is the device's committed `transferResult`. The queue
    /// sends it **last**, after every member route (spec §7.7).
    func uploadTrip(_ trip: TripBlob) -> TransferHandle
    /// Delete a trip object from the device (`deleteObject` for a trip, spec §4.4)
    /// — **non-cascading**: only the trip metadata goes, its member routes stay.
    /// The "Delete trip & routes" cascade is composed by the caller (per-route
    /// deletes + this).
    func deleteTrip(_ id: DeviceObjectID) async throws
    /// Enumerate tracked rides on the device — the summaries plus the v2 header's
    /// truncation signal (`RideCatalog.hiddenRideCount`, spec §7.4).
    func listRides() async throws -> RideCatalog
    /// Full detail for one tracked ride (E3): the elevation profile.
    func rideDetail(_ id: RideID) async throws -> RideDetail
    /// Download tracked rides (device → app, B7). `rides` yields each ride's
    /// compact-binary payload as it lands; `handle` carries batch progress /
    /// cancel / restart (whole rides are the resume granularity).
    func downloadRides(_ ids: [RideID]) -> RideDownload
    /// Ack the rides the phone's library holds (`ackRides`, spec §4.4 cmd 2) —
    /// the device reconciles its per-ride "synced" flag from this possession
    /// list (monotonic: it only ever *sets* flags). Idempotent and order-free,
    /// so callers re-send the whole list on every connect; the transport chunks
    /// a long list across writes. Ids outside the device namespace (mock/test
    /// ids that never came from a catalog) are skipped.
    func ackRides(_ ids: [RideID]) async throws
}

/// The aggregate device boundary (Tier 1 — semantic), composed from the
/// capability protocols that focused policies and view models use directly.
/// No caller reaches through it to CoreBluetooth. Two aggregate conformers:
///
///   • `BLETransport`  (real, this module) — CoreBluetooth + the `BLEChannel` byte layer.
///   • `MockTransport` (fake, `#if DEBUG`) — fixtures + fault injection (B1M).
///
/// Everything a screen (B2–B11) needs must be expressible through these
/// capabilities. `Sendable` lets conformers cross concurrency domains.
public protocol DeviceTransport: DeviceLink, DeviceConfiguration, DeviceObjects {
    // MARK: Control plane (GATT — DIS / BAS / OBC Control)

    /// Battery percentage (BAS notify). **Replays the latest** value.
    var battery: AsyncStream<Int> { get }
    /// Unsolicited device store movements (`storeChanged`, spec §4.3 msg 2) —
    /// an object committed or deleted **on the device** while connected (the
    /// on-device route delete, epic #447 P6). **Live edges only, no replay**:
    /// a movement is an event, not a state — late subscribers reconcile via
    /// their own connect-time reload, never against a stale edge. Consumers
    /// also audit catalogs at low cadence while connected because a BLE notify
    /// can be dropped without replay.
    var storeChanges: AsyncStream<StoreChanged> { get }
    /// Stamp the device's **trusted wall clock** (`setClock`, spec §4.4 cmd 5,
    /// epic #638). Sent on **every connect, after encryption and before the first
    /// `ackRides` / reconcile write** — the device has no RTC, and this (or a GPS
    /// fix) is what marks its clock trusted for the boot, the retention sweep's
    /// safety gate. Returns ``ClockSyncOutcome/unsupported`` for a device that
    /// predates expiry (`commandResult(unknownCommand)`): a supported peer, not an
    /// error — the app hides expiry UI and sends no retention. Throws only on a
    /// link/write failure.
    func setClock(_ sample: WallClockSample) async throws -> ClockSyncOutcome
    /// Set a stored route's **retention level** (`setRouteRetention`, spec §4.4
    /// cmd 6, epic #638) without re-uploading it — sent after an upload commit and
    /// whenever the desired level diverges from the device's at reconcile. The
    /// device writes the level without touching `last_used`. Returns
    /// ``RetentionWriteOutcome/notFound`` for an id the device no longer holds and
    /// ``RetentionWriteOutcome/unsupported`` for a pre-expiry device. Throws only
    /// on a link/write failure.
    func setRouteRetention(_ id: DeviceObjectID, _ retention: Retention) async throws -> RetentionWriteOutcome
    /// Read the device diagnostics/crash-log blob.
    func readDiagnostics() async throws -> Data

    /// Ask the device to dissolve **its** side of the bond (`forgetBond`, spec
    /// §4.4 cmd 4). The app's "Forget device" otherwise clears only the phone's
    /// `BondRecord`; the device keeps its bond, and the reject-when-bonded posture
    /// (spec §8) then refuses every new pairing until the rider also runs Forget
    /// phone on the device — a one-sided forget leaves the pair wedged. This
    /// command, honoured **only over the already-encrypted bonded link** (the
    /// bonded phone asking to clear its own bond is fully consistent with
    /// reject-when-bonded — a stranger can never issue it), makes the device clear
    /// its bond and return to open-pairing advertising. **Best-effort**: the
    /// device answers `commandResult(ok)` then drops the link, so the transport
    /// waits only briefly for the ack; the caller (Settings forget) clears its
    /// local record whether this succeeds, times out, or throws. Invoke it only
    /// while connected — an offline forget can't reach the device (it keeps its
    /// bond until the rider forgets the phone on it).
    func forgetBond() async throws

    // MARK: Weather (spec §11 — the standing watch)

    /// Arm or disarm the **standing weather watch** (WX9): a UUID-filtered scan for the bonded
    /// device's Weather Request advertisement whenever nothing else needs the radio, so a device
    /// raising a request wakes the app — foregrounded, backgrounded, or after the process was
    /// killed (CoreBluetooth state restoration). The flag persists across relaunches.
    ///
    /// On the protocol rather than only on `BLETransport` because the rider owns it now: WX13's
    /// *Background weather* switch is the first caller that ever passes `false`, and a view model
    /// may not reach past `DeviceTransport` to find one (the golden rule). Stand-ins that model no
    /// radio ignore it, which is the truthful stand-in behaviour — there is no scan to arm.
    func setWeatherWatch(_ enabled: Bool)

    // MARK: Firmware update (S7 — DFU delivery)

    /// Upload a firmware update (app → device, S7). The payload is the whole OBCU
    /// container (spec §7.6 — `fwImage` type 5, the **singleton** object id `0`):
    /// progress + cancel + whole-object restart, exactly like a route upload. A
    /// CRC-verified commit promotes the bytes to `/UPDATE.BIN` on the card,
    /// replacing any existing one; a torn transfer never becomes a visible file.
    /// Staging never installs — that's `installFirmware()`.
    func uploadFirmware(_ container: Data) -> TransferHandle
    /// Ask the device to install the staged `/UPDATE.BIN` (`installFw`, spec §4.4
    /// cmd 3). The command only *requests*: the device runs its on-glass check →
    /// confirm flow and installs only on a physical Select press. Returns the
    /// mapped request outcome (`accepted` opens that flow); throws only on a link
    /// failure (`notConnected` / `writeFailed`), never on a device reply.
    func installFirmware() async throws -> FirmwareInstallResult
}

extension DeviceLink {
    /// Default single-phase behaviour for conformers that don't split pairing
    /// (SwiftUI previews, future stand-ins): `discover()` does the whole connect
    /// and `authenticate()` is a no-op. `BLETransport` and `MockTransport` override
    /// both to defer the gated ops past the D2 row tap (#297).
    public func discover() async throws { try await connect() }
    public func authenticate() async throws {}

    /// Default foreground-only lifecycle (#459) for conformers without their own
    /// reconnect machinery (the mock, previews): suspending is a plain teardown
    /// and resuming replays the full connect, errors swallowed (a background
    /// reconnect is silent — the S4 banner tells the degraded-link story).
    /// `BLETransport` overrides `resumeLink()`: its reconnect is a re-armed
    /// intent latch, not a fresh `connect()` (which would park new
    /// discover/authenticate continuations over any still waiting).
    public func suspendLink() async { await disconnect() }
    public func resumeLink() async { try? await connect() }
}

extension DeviceObjects {
    /// Default: no possession ack — for preview/test stand-ins that model no
    /// device-side synced state. `BLETransport` sends the real command;
    /// `MockTransport` records the ack for tests. Safe as a no-op because the
    /// ack is pure reconciliation — skipping it only leaves the device's
    /// synced flags where they were.
    public func ackRides(_ ids: [RideID]) async throws {}
}

extension DeviceTransport {
    /// Default: the device can't stamp its clock — for preview/test stand-ins that
    /// don't model expiry (a device predating `setClock` reads the same way, spec
    /// §4.4 compat). Reads as `unsupported` so S7 hides expiry UI and no retention
    /// is sent. `BLETransport` sends the real command; `MockTransport` records it.
    public func setClock(_ sample: WallClockSample) async throws -> ClockSyncOutcome { .unsupported }

    /// Default: retention can't be set — the same pre-expiry stand-in posture as
    /// `setClock`. Safe as `unsupported`: the reconcile/upload push gate on the
    /// capability, so a stand-in simply pushes nothing.
    public func setRouteRetention(
        _ id: DeviceObjectID, _ retention: Retention
    ) async throws -> RetentionWriteOutcome { .unsupported }

    /// Default: no firmware delivery — for preview/test stand-ins that don't model
    /// DFU. `BLETransport` streams the real `fwImage`; `MockTransport` paces a
    /// fixture transfer. An update offered against such a stand-in fails as "no
    /// link" rather than trapping.
    public func uploadFirmware(_ container: Data) -> TransferHandle {
        .immediatelyFinished(.failed(.notConnected))
    }

    /// Default: the device can't be updated over Bluetooth — for stand-ins that
    /// don't model the `installFw` command (a device predating BLE DFU reads the
    /// same way, spec §4.4 compat).
    public func installFirmware() async throws -> FirmwareInstallResult { .unsupported }

    /// Default: no device-side bond to dissolve — for preview/test stand-ins
    /// (a device predating `forgetBond` reads the same way, spec §4.4 compat).
    /// Safe as a no-op because it's pure best-effort: skipping it only leaves the
    /// device's bond where it was, which the caller's local-record clear already
    /// tolerates. `BLETransport` sends the real command; `MockTransport` records
    /// the request.
    public func forgetBond() async throws {}

    /// Default: no radio, so no watch to arm — for preview/test stand-ins. Safe as a no-op in a
    /// way the other defaults are not merely conveniently: the watch *is* a scan, and a transport
    /// that does not scan has nothing to turn off. The rider's preference is stored by
    /// ``WeatherPreferencesStore`` either way, so the setting survives a stand-in run.
    public func setWeatherWatch(_ enabled: Bool) {}
}

extension DeviceObjects {
    // MARK: Trips (TR8) — defaults for stand-ins that don't model trips
    //
    // A preview/test transport that predates trips (or doesn't care) reads as a
    // device with an empty trip catalog and no trip transfer support — the same
    // way a v1 peer would (spec §4.4 forward-compat). `BLETransport` and
    // `MockTransport` override all four with the real trip object plane.

    /// Default: no trips on the device (empty catalog).
    public func listTrips() async throws -> [TripCatalogEntry] { [] }
    /// Default: the trip object can't be downloaded (no trip store).
    public func downloadTrip(_ id: DeviceObjectID) async throws -> TripObjectCodec.Decoded {
        throw DeviceError.readFailed
    }
    /// Default: trip upload isn't supported — reads as "no link" rather than
    /// trapping (the same as the firmware/ride download stand-in defaults).
    public func uploadTrip(_ trip: TripBlob) -> TransferHandle {
        .immediatelyFinished(.failed(.notConnected))
    }
    /// Default: nothing to delete (no trip store) — a best-effort no-op, like
    /// `forgetBond` / `ackRides`.
    public func deleteTrip(_ id: DeviceObjectID) async throws {}
}
