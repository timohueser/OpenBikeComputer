import Foundation
import OBCDomain

/// The spine of the app. **Every view model depends only on this protocol**,
/// never on CoreBluetooth. Two conformers:
///
///   • `BLETransport`  (real, this module) — CoreBluetooth + the `BLEChannel` byte layer.
///   • `MockTransport` (fake, `#if DEBUG`) — fixtures + fault injection.
///
/// If a screen needs device data, it comes through `DeviceTransport`, never a
/// CoreBluetooth detour. `Sendable` so conformers can be actors or
/// `@unchecked Sendable` classes and still cross concurrency domains.
public protocol DeviceTransport: Sendable {
    // MARK: Lifecycle

    /// Link lifecycle. **Replays the latest** value to late subscribers (a fresh
    /// stream immediately yields the current state), then streams changes.
    var state: AsyncStream<ConnectionState> { get }
    /// Begin connecting: power-on wait, scan, connect, discover, open the CoC.
    /// Throws `DeviceError` on failure (never traps). The full link = `discover()`
    /// then `authenticate()`. The protocol-version check runs where `deviceInfo()`
    /// is consumed on connect — a mismatch surfaces as a banner + disabled sync,
    /// not a thrown connect (which would mis-degrade the connection state).
    func connect() async throws
    /// **First-time-pairing phase 1**: power-on wait, scan, connect, and discover
    /// services + only the **un-gated** characteristics (DIS / BAS /
    /// `protocolVersion`) — enough for `deviceInfo()` and the device row, but
    /// touching **no** gated characteristic, so iOS does *not* raise the LESC
    /// passkey sheet yet. The gated ops wait for `authenticate()`.
    func discover() async throws
    /// **First-time-pairing phase 2**: the gated operations that establish the
    /// encrypted, LESC-authenticated link — subscribe the `status` /
    /// `transferControl` notifies, read the PSM, open the CoC. This is what raises
    /// the system passkey sheet; the launch flow calls it on the device-row tap so
    /// the sheet lands in the "pairing…" beat. Requires a prior `discover()`.
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
    // Ids on this plane are **device-namespace**: a `RouteID`/`RideID` whose
    // rawValue is the decimal object id the device's list objects enumerate
    // (spec §4.1 — durable for the life of the stored object). Library ids
    // never cross this boundary; `PlannedRouteRecord.deviceObjectID` is the
    // app's durable link between the two.

    /// Enumerate routes stored on the device — reconcile input for the
    /// "on device" badge, never Planned-list rows.
    func listRoutes() async throws -> [RouteSummary]
    /// Full detail for one stored route: the stored OBCR object, decoded
    /// app-side (spec §7.1 — "download the route object").
    func routeDetail(_ id: RouteID) async throws -> RouteDetail
    /// Upload a route (app → device). Success is the device's committed
    /// `transferResult`; `resume()` after a drop restarts the whole upload.
    func uploadRoute(_ route: RouteBlob) -> TransferHandle
    /// Delete a route from the device.
    func deleteRoute(_ id: RouteID) async throws
    /// Enumerate tracked rides on the device.
    func listRides() async throws -> [RideSummary]
    /// Full detail for one tracked ride: the elevation profile.
    func rideDetail(_ id: RideID) async throws -> RideDetail
    /// Download tracked rides (device → app). `rides` yields each ride's
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
    /// both to defer the gated ops past the device-row tap.
    public func discover() async throws { try await connect() }
    public func authenticate() async throws {}
}
