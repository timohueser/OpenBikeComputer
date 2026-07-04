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

    // MARK: Control plane (GATT — DIS / BAS / OBC Control)

    /// Device identity (DIS + `protocol_version`).
    func deviceInfo() async throws -> DeviceInfo
    /// Battery percentage (BAS notify). **Replays the latest** value.
    var battery: AsyncStream<Int> { get }
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
}
