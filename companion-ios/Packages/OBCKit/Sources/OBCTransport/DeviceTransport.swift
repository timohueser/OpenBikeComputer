import Foundation
import OBCDomain

/// The spine of the app (Tier 1 — semantic). **Every view model depends only on
/// this protocol**, never on CoreBluetooth. Two conformers:
///
///   • `BLETransport`  (real, this module) — CoreBluetooth + the `BLEChannel` byte layer.
///   • `MockTransport` (fake, `#if DEBUG`) — fixtures + fault injection (B1M).
///
/// Everything a screen (B2–B11) needs must be expressible here — if a screen needs
/// data, it comes through `DeviceTransport`, never a CoreBluetooth detour.
/// `Sendable` so conformers can be actors or `@unchecked Sendable` classes and
/// still cross concurrency domains.
public protocol DeviceTransport: Sendable {
    // MARK: Lifecycle

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
    /// the encrypted, LESC-authenticated link — subscribe the `status` /
    /// `transferControl` notifies, read the PSM, open the CoC. This is what raises
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

    // MARK: Control plane (GATT — DIS / BAS / OBC Control)

    /// Device identity (DIS + `protocol_version`).
    func deviceInfo() async throws -> DeviceInfo
    /// Battery percentage (BAS notify). **Replays the latest** value.
    var battery: AsyncStream<Int> { get }
    /// Unsolicited device store movements (`storeChanged`, spec §4.3 msg 2) —
    /// an object committed or deleted **on the device** while connected (the
    /// on-device route delete, epic #447 P6). **Live edges only, no replay**:
    /// a movement is an event, not a state — late subscribers reconcile via
    /// their own connect-time reload, never against a stale edge.
    var storeChanges: AsyncStream<StoreChanged> { get }
    /// Read the device config blob.
    func readConfig() async throws -> DeviceConfig
    /// Write the device config blob — including device rename (H3, Delta 1).
    func writeConfig(_ config: DeviceConfig) async throws

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
    /// Enumerate tracked rides on the device.
    func listRides() async throws -> [RideSummary]
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
    /// Read the device diagnostics/crash-log blob.
    func readDiagnostics() async throws -> Data
}

extension DeviceTransport {
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

    /// Default: no possession ack — for preview/test stand-ins that model no
    /// device-side synced state. `BLETransport` sends the real command;
    /// `MockTransport` records the ack for tests. Safe as a no-op because the
    /// ack is pure reconciliation — skipping it only leaves the device's
    /// synced flags where they were.
    public func ackRides(_ ids: [RideID]) async throws {}
}
