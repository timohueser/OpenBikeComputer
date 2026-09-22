import Foundation
import OBCDomain

public enum ClockSyncOutcome: Equatable, Sendable {
    case stamped
    case unsupported
}
/// The device link lifecycle and identity handshake, without unrelated capabilities.
public protocol DeviceLink: Sendable {
    /// Link lifecycle. Replays the latest value to a late subscriber, then streams changes.
    var state: AsyncStream<ConnectionState> { get }
    /// Begin connecting: power-on wait, scan, connect, discover, open the CoC. Throws a
    /// `DeviceError` on failure, never traps. The full link is `discover()` then `authenticate()`.
    func connect() async throws
    /// First-time-pairing phase 1: scan, connect, and discover the services and only the un-gated
    /// characteristics, enough for `deviceInfo()` and the device row. It touches no gated
    /// characteristic, so iOS does not raise the passkey sheet yet.
    func discover() async throws
    /// First-time-pairing phase 2: the gated operations that establish the encrypted,
    /// LESC-authenticated link. This is what raises the system passkey sheet, so the launch flow
    /// calls it on the device-row tap. Requires a prior `discover()`.
    func authenticate() async throws
    /// Tear the link down.
    func disconnect() async
    /// Drop the link and pause the transport's own reconnect behaviour until `resumeLink()`. The
    /// app calls this on a real background transition: a transport that kept scanning, or held a
    /// pending connect, would fight the intentional disconnect and re-raise the link.
    func suspendLink() async
    /// Undo `suspendLink()` by re-arming the reconnect machinery: the existing bonded
    /// silent-reconnect path, never a fresh pairing flow. Failure is silent, and callers invoke
    /// this only when a link existed before the suspend.
    func resumeLink() async
    /// Device identity established by discovery.
    func deviceInfo() async throws -> DeviceInfo
}

/// Battery telemetry, without link lifecycle or mutation authority.
public protocol DeviceBattery: Sendable {
    /// Battery percentage. Replays the latest value.
    var battery: AsyncStream<Int> { get }
}

/// The device configuration control plane, separated from link and object
/// transport so config-only policies do not acquire unrelated capabilities.
public protocol DeviceConfiguration: Sendable {
    func readConfig() async throws -> DeviceConfig
    /// Write the device config blob, including a device rename.
    func writeConfig(_ config: DeviceConfig) async throws
}

/// Device-side bond administration, without unrelated feature authority.
public protocol DeviceBonding: Sendable {
    /// Ask the device to dissolve its side of the bond. Without it, "Forget device" clears only
    /// the phone's `BondRecord`, the device keeps its bond, and its reject-when-bonded posture
    /// then refuses every new pairing until the rider also runs Forget phone on the device.
    /// Honoured only over the already-encrypted bonded link, so a stranger can never issue it.
    /// Best-effort: the device answers, then drops the link, so the caller clears its local record
    /// whether this succeeds, times out or throws. Invoke it only while connected.
    func forgetBond() async throws
}

public protocol DeviceClock: Sendable {
    func setClock(_ sample: WallClockSample) async throws -> ClockSyncOutcome
}

/// An in-process catalog invalidation cue used by mocks and preview transports. The real BLE
/// transport reconciles catalog state through LIST and STATUS and the connected audit instead.
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
    /// Optional local invalidation edges. The BLE implementation is a finished stream, and its
    /// consumers reconcile on connect and audit while connected.
    var catalogChanges: AsyncStream<CatalogChange> { get }

    // MARK: Data plane (bulk objects)
    //
    // Ids on this plane are device-namespace (`DeviceObjectID`), durable for the life of the
    // stored object. Route ops take one directly; ride ids are minted from the catalog.

    /// Enumerate routes stored on the device: reconcile input for the "on device" badge, never
    /// Planned-list rows.
    func listRoutes() async throws -> [RouteCatalogEntry]
    /// Full detail for one stored route: the stored object, decoded app-side.
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail
    /// Upload a route. `resume()` after a drop restarts the whole upload.
    func uploadRoute(_ route: RouteBlob) -> TransferHandle
    func deleteRoute(_ id: DeviceObjectID) async throws
    /// Enumerate trips stored on the device. Reconcile input for the trip card's badge, where the
    /// per-entry `crc32` is the fingerprint; never list rows.
    func listTrips() async throws -> [TripCatalogEntry]
    /// The stored trip object: its name and stage device ids in ride order, dangling refs
    /// included. Reconcile fetches this only when the catalog fingerprint cannot decide.
    func downloadTrip(_ id: DeviceObjectID) async throws -> TripObjectCodec.Trip
    /// Upload a whole trip object, the trip sibling of `uploadRoute`. A fresh trip lets the device
    /// mint an id; a re-push or an adoption sends the stored id to replace it in place. The queue
    /// sends this last, after every member route.
    func uploadTrip(_ trip: TripBlob) -> TransferHandle
    /// Delete a trip object. Non-cascading: only the trip metadata goes, its member routes stay.
    /// The caller composes the "Delete trip & routes" cascade from per-route deletes and this.
    func deleteTrip(_ id: DeviceObjectID) async throws
    /// Enumerate tracked rides on the device, including the bounded-catalog truncation signal.
    func listRides() async throws -> RideCatalog
    /// Full detail for one tracked ride.
    func rideDetail(_ id: RideID) async throws -> RideDetail
    /// Download tracked rides. `rides` yields each ride's payload as it lands; `handle` carries
    /// batch progress, cancel and restart, and whole rides are the resume granularity.
    func downloadRides(_ ids: [RideID]) -> RideDownload
    func downloadRides(from rides: [RideSummary]) -> RideDownload
    /// Confirm exact durable client possession. Local sync counts are not this confirmation.
    func confirmRideArchive(_ receipt: RideArchiveReceipt) async throws -> RideArchiveConfirmation
}

public protocol DeviceUpdates: Sendable {
    // MARK: Firmware update

    /// Upload a firmware update: the whole OBCU container, with progress, cancel and whole-object
    /// restart, exactly like a route upload. A verified commit replaces any staged file already
    /// there, and a torn transfer never becomes a visible one. Staging never installs.
    func uploadFirmware(_ container: Data) -> TransferHandle
    /// Ask the device to install the staged update. The command only requests: the device runs its
    /// own check and confirm flow and installs only on a physical press. Returns the mapped
    /// outcome, and throws only on a link failure, never on a device reply.
    func installFirmware() async throws -> FirmwareInstallResult
}

public protocol DeviceTransport: DeviceLink, DeviceBattery, DeviceConfiguration,
    DeviceBonding, DeviceObjects, DeviceClock, DeviceUpdates {
    // MARK: Control plane
    func readDiagnostics() async throws -> Data

}

extension DeviceLink {
    /// Default single-phase behaviour for conformers that do not split pairing: `discover()` does
    /// the whole connect and `authenticate()` is a no-op.
    public func discover() async throws { try await connect() }
    public func authenticate() async throws {}

    /// Default foreground-only lifecycle for conformers without their own reconnect machinery:
    /// suspending is a plain teardown, resuming replays the full connect, and errors are
    /// swallowed. `BLETransport` overrides `resumeLink()`, because its reconnect is a re-armed
    /// intent latch, not a fresh `connect()` that would park new continuations.
    public func suspendLink() async { await disconnect() }
    public func resumeLink() async { try? await connect() }
}

extension DeviceObjects {
    /// Default: no local catalog edge stream.
    public var catalogChanges: AsyncStream<CatalogChange> { AsyncStream { $0.finish() } }

}

extension DeviceUpdates {
    /// Default: no firmware delivery, for stand-ins that do not model it. An update offered
    /// against such a stand-in fails as "no link" rather than trapping.
    public func uploadFirmware(_ container: Data) -> TransferHandle {
        .immediatelyFinished(.failed(.notConnected))
    }

    /// Default: the device cannot be updated over Bluetooth.
    public func installFirmware() async throws -> FirmwareInstallResult { .unsupported }
}

extension DeviceBonding {
    /// Default: no device-side bond to dissolve. Safe as a no-op because it is pure best-effort:
    /// skipping it only leaves the device's bond where it was, which the caller's local-record
    /// clear already tolerates.
    public func forgetBond() async throws {}
}

extension DeviceClock {
    public func setClock(_ sample: WallClockSample) async throws -> ClockSyncOutcome { .unsupported }
}

extension DeviceTransport {

}

extension DeviceObjects {
    // MARK: Trips
    //
    // Defaults for stand-ins that do not model trips: an empty trip catalog and no transfers.

    public func listTrips() async throws -> [TripCatalogEntry] { [] }
    public func downloadTrip(_ id: DeviceObjectID) async throws -> TripObjectCodec.Trip {
        throw DeviceError.readFailed
    }
    /// Default: trip upload reads as "no link" rather than trapping.
    public func uploadTrip(_ trip: TripBlob) -> TransferHandle {
        .immediatelyFinished(.failed(.notConnected))
    }
    /// Default: nothing to delete, a best-effort no-op.
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
