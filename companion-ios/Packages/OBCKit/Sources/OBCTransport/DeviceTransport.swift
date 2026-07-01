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
    /// Begin connecting: power-on wait, scan, connect, discover, open the CoC, and
    /// run the protocol-version check. Throws `DeviceError` on failure (never traps).
    func connect() async throws
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

    // MARK: Data plane (bulk objects — progress + cancel + resume)

    /// Enumerate routes stored on the device.
    func listRoutes() async throws -> [RouteSummary]
    /// Upload a route (app → device, B5). Returns a handle for progress/cancel/resume.
    func uploadRoute(_ route: RouteBlob) -> TransferHandle
    /// Delete a route from the device.
    func deleteRoute(_ id: RouteID) async throws
    /// Enumerate tracked rides on the device.
    func listRides() async throws -> [RideSummary]
    /// Download tracked rides (device → app, B7). `rides` yields each ride's
    /// compact-binary payload as it lands; `handle` carries batch
    /// progress/cancel/resume.
    func downloadRides(_ ids: [RideID]) -> RideDownload
    /// Read the device diagnostics/crash-log blob.
    func readDiagnostics() async throws -> Data
}
