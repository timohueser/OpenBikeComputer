import Foundation
import OBCDomain

public enum ClockSyncOutcome: Equatable, Sendable {
    /// The device stamped its trusted clock (`commandResult(ok)`).
    case stamped
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
    /// the encrypted, LESC-authenticated link — subscribe the v4 object-control indication,
    /// read the PSM, and open the CoC. BLE imperative commands arm their separate `status`
    /// notification lazily after this phase. This is what raises
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

/// Battery telemetry, without link lifecycle or mutation authority.
public protocol DeviceBattery: Sendable {
    /// Battery percentage (BAS notify). **Replays the latest** value.
    var battery: AsyncStream<Int> { get }
}

/// The device configuration control plane, separated from link and object
/// transport so config-only policies do not acquire unrelated capabilities.
public protocol DeviceConfiguration: Sendable {
    /// Read the device config blob.
    func readConfig() async throws -> DeviceConfig
    /// Write the device config blob — including device rename (H3, Delta 1).
    func writeConfig(_ config: DeviceConfig) async throws
}

/// Device-side bond administration, without unrelated feature authority.
public protocol DeviceBonding: Sendable {
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
}

public protocol DeviceClock: Sendable {
    func setClock(_ sample: WallClockSample) async throws -> ClockSyncOutcome
}

/// An in-process catalog invalidation cue used by mocks and preview transports. Protocol v4 has no
/// v2 catalog-edge notification; the real BLE transport reconciles catalog state through `LIST` /
/// `STATUS` and the connected audit instead.
public struct CatalogChange: Equatable, Sendable {
    public enum Kind: Equatable, Sendable {
        case route
        case ride
        case trip
    }

    public var kind: Kind

    public init(kind: Kind) {
        self.kind = kind
    }
}

public protocol DeviceObjects: Sendable {
    /// Optional local invalidation edges. The BLE implementation is a finished stream because v4
    /// removed the v2 notification; consumers reconcile on connect and audit while connected.
    var catalogChanges: AsyncStream<CatalogChange> { get }

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
    /// Upload a route (app → device, B5). Success is a reconciled protocol-v4 `PUT` commit;
    /// `resume()` after a drop restarts the whole upload.
    func uploadRoute(_ route: RouteBlob) -> TransferHandle
    /// Delete a route from the device.
    func deleteRoute(_ id: DeviceObjectID) async throws
    /// Enumerate trips stored on the device from protocol-v4 catalog metadata. Reconcile input for the
    /// trip card's "on device" badge (the per-entry `crc32` is the fingerprint),
    /// never list rows (trips are library-first, like routes).
    func listTrips() async throws -> [TripCatalogEntry]
    /// Full contents of one stored trip object (spec §7.7 — "download the trip
    /// object"): the name + the stage device ids in ride order, dangling refs
    /// included. Reconcile only fetches this when the catalog fingerprint can't
    /// decide (the primary check is the entry's `crc32`).
    func downloadTrip(_ id: DeviceObjectID) async throws -> TripObjectCodec.Decoded
    /// Upload a whole trip object (app → device, TR8) — the trip sibling of
    /// `uploadRoute`. A fresh trip sends `0xFFFF` (the device mints an id from its
    /// own trip counter); a re-push / adoption sends the stored id to replace it
    /// in place. Success is the device's reconciled protocol-v4 `PUT` commit. The queue
    /// sends it **last**, after every member route (spec §7.7).
    func uploadTrip(_ trip: TripBlob) -> TransferHandle
    /// Delete a trip object from the device (`deleteObject` for a trip, spec §4.4)
    /// — **non-cascading**: only the trip metadata goes, its member routes stay.
    /// The "Delete trip & routes" cascade is composed by the caller (per-route
    /// deletes + this).
    func deleteTrip(_ id: DeviceObjectID) async throws
    /// Enumerate tracked rides on the device, including the bounded-catalog truncation signal.
    func listRides() async throws -> RideCatalog
    /// Full detail for one tracked ride (E3): the elevation profile.
    func rideDetail(_ id: RideID) async throws -> RideDetail
    /// Download tracked rides (device → app, B7). `rides` yields each ride's
    /// compact-binary payload as it lands; `handle` carries batch progress /
    /// cancel / restart (whole rides are the resume granularity).
    func downloadRides(_ ids: [RideID]) -> RideDownload
    func downloadRides(from rides: [RideSummary]) -> RideDownload
    /// Confirm exact durable client possession. Local sync counts are not this confirmation.
    func confirmRideArchive(_ receipt: RideArchiveReceipt) async throws -> RideArchiveConfirmation
}

public protocol DeviceUpdates: Sendable {
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

public protocol DeviceTransport: DeviceLink, DeviceBattery, DeviceConfiguration,
    DeviceBonding, DeviceObjects, DeviceClock, DeviceUpdates {
    // MARK: Control plane (GATT — DIS / BAS / OBC Control)
    /// Read the device diagnostics/crash-log blob.
    func readDiagnostics() async throws -> Data

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
    /// Default: no local catalog edge stream.
    public var catalogChanges: AsyncStream<CatalogChange> { AsyncStream { $0.finish() } }

}

extension DeviceUpdates {
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
}

extension DeviceBonding {
    /// Default: no device-side bond to dissolve — for preview/test stand-ins
    /// (a device predating `forgetBond` reads the same way, spec §4.4 compat).
    /// Safe as a no-op because it's pure best-effort: skipping it only leaves the
    /// device's bond where it was, which the caller's local-record clear already
    /// tolerates. `BLETransport` sends the real command; `MockTransport` records
    /// the request.
    public func forgetBond() async throws {}
}

extension DeviceClock {
    public func setClock(_ sample: WallClockSample) async throws -> ClockSyncOutcome { .unsupported }
}

extension DeviceTransport {

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
    /// `forgetBond`.
    public func deleteTrip(_ id: DeviceObjectID) async throws {}
}

// Stand-in transports do not carry device source identities.
extension DeviceObjects {
    public func downloadRides(from rides: [RideSummary]) -> RideDownload {
        downloadRides(rides.map(\.id))
    }
}

/// Terminal device dispositions; transport or media failures throw and can be retried later.
public enum RideArchiveConfirmation: Equatable, Sendable {
    case confirmed
    case sourceUnavailable
    case unsupported
    case refused
}

extension DeviceObjects {
    public func confirmRideArchive(_ receipt: RideArchiveReceipt) async throws -> RideArchiveConfirmation {
        .unsupported
    }
}
