import Foundation
import OBCDomain

/// The phone-side library: planned routes saved from imports and tracked rides
/// synced off the device — what keeps the lists browsable before, without, or
/// away from a device, and what makes re-sync idempotent.
///
/// A seam like `DeviceTransport`: screens and flows see only this protocol; the
/// composition root picks the conformer — `FileLibraryStore` (real),
/// `InMemoryLibraryStore` (tests/previews, and mock runs so every XCUITest
/// launch starts from its scenario alone).
///
/// Stores hold **canonical domain models** (`PlannedRouteRecord`, `Ride`) — never
/// device wire bytes, whose layout is firmware-owned. Calls are synchronous and
/// expected from the main actor (payloads are small; can move off-main if ride
/// tracklogs grow).
public protocol LibraryStore: Sendable {
    // MARK: Planned routes

    /// Every saved planned route, newest first (`addedAt` descending).
    func plannedRoutes() -> [PlannedRouteRecord]
    /// Insert or replace a record under its `summary.id` — also the write path
    /// for renames and the uploaded-to-device flip.
    func savePlannedRoute(_ record: PlannedRouteRecord)
    func deletePlannedRoute(_ id: RouteID)

    // MARK: Tracked rides (sync)

    /// Every synced ride, newest first (`summary.date` descending).
    func rides() -> [Ride]
    /// Insert or replace a ride under its id — called per ride as a sync lands
    /// it, so an interrupted batch keeps its partial across a relaunch.
    func saveRide(_ ride: Ride)
    func deleteRide(_ id: RideID)

    /// Every ride id this phone has ever downloaded. **Survives `deleteRide`**,
    /// so a deleted ride is never re-counted as "new" (idempotent re-sync).
    func syncedRideIDs() -> Set<RideID>
    func markRideSynced(_ id: RideID)

    /// Ride ids the user deleted *on the phone*. The device keeps its copy
    /// (the SD card is untouched), so the list merge must hide these device
    /// rides instead of resurrecting them on every sync/reload.
    func deletedRideIDs() -> Set<RideID>
    func markRideDeleted(_ id: RideID)
}

/// The no-filesystem conformer: unit tests, previews, and Debug mock runs.
/// Persistence across relaunches is `FileLibraryStore`'s job; scenario-driven
/// launches must start from their fixtures alone.
public final class InMemoryLibraryStore: LibraryStore, @unchecked Sendable {
    private let lock = NSLock()
    private var planned: [RouteID: PlannedRouteRecord] = [:]
    private var rideRecords: [RideID: Ride] = [:]
    private var synced: Set<RideID> = []
    private var deleted: Set<RideID> = []

    public init() {}

    public func plannedRoutes() -> [PlannedRouteRecord] {
        lock.withLock { planned.values.sorted { $0.addedAt > $1.addedAt } }
    }

    public func savePlannedRoute(_ record: PlannedRouteRecord) {
        lock.withLock { planned[record.id] = record }
    }

    public func deletePlannedRoute(_ id: RouteID) {
        lock.withLock { planned[id] = nil }
    }

    public func rides() -> [Ride] {
        lock.withLock { rideRecords.values.sorted { $0.summary.date > $1.summary.date } }
    }

    public func saveRide(_ ride: Ride) {
        lock.withLock { rideRecords[ride.id] = ride }
    }

    public func deleteRide(_ id: RideID) {
        lock.withLock { rideRecords[id] = nil }
    }

    public func syncedRideIDs() -> Set<RideID> {
        lock.withLock { synced }
    }

    public func markRideSynced(_ id: RideID) {
        lock.withLock { _ = synced.insert(id) }
    }

    public func deletedRideIDs() -> Set<RideID> {
        lock.withLock { deleted }
    }

    public func markRideDeleted(_ id: RideID) {
        lock.withLock { _ = deleted.insert(id) }
    }
}
