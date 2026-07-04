import Foundation
import OBCDomain

/// The phone-side library (B1S): planned routes saved from imports and tracked
/// rides synced off the device — what keeps the lists browsable before, without,
/// or away from a device (H4 save-before-pairing, the S4 offline rule) and what
/// makes B7's re-sync idempotent (H9/H10).
///
/// A seam like `DeviceTransport`: screens and flows see only this protocol; the
/// composition root picks the conformer — `FileLibraryStore` (real),
/// `InMemoryLibraryStore` (tests/previews, and mock runs so every XCUITest
/// launch starts from its scenario alone).
///
/// Stores hold **canonical domain models** (`PlannedRouteRecord`, `Ride`) — never
/// device wire bytes, whose layout is firmware-`S0`-owned (see #256). Calls are
/// synchronous and expected from the main actor. Ride reads are split (#360):
/// the lists browse `rideSummaries()` (small), and a tracklog loads one ride at
/// a time via `ridePoints(_:)` when a detail opens — a season of rides must
/// never be decoded whole at launch.
public protocol LibraryStore: Sendable {
    // MARK: Planned routes (imports, H4)

    /// Every saved planned route, newest first (`addedAt` descending).
    func plannedRoutes() -> [PlannedRouteRecord]
    /// Insert or replace a record under its `summary.id` — also the write path
    /// for renames (H12) and the uploaded-to-device flip.
    func savePlannedRoute(_ record: PlannedRouteRecord)
    func deletePlannedRoute(_ id: RouteID)

    // MARK: Tracked rides (sync, B7)

    /// Every synced ride's summary, newest first (`date` descending) — the
    /// Tracked list's whole appetite. Never decodes tracklogs (#360).
    func rideSummaries() -> [RideSummary]
    /// One ride's full tracklog, loaded on demand (the detail map's read).
    /// `nil` when the ride is unknown or its points don't decode — the ride
    /// stays summary-only rather than dropped, and the detail degrades to the
    /// preview's coordinates.
    func ridePoints(_ id: RideID) -> [RidePoint]?
    /// Insert or replace a ride under its id — called per ride as a sync lands
    /// it, so an interrupted batch (H10) keeps its partial across a relaunch.
    func saveRide(_ ride: Ride)
    /// Update a ride's summary without touching its stored points — the rename
    /// (H12) write path; re-encoding a full tracklog to change a name would be
    /// the exact whole-ride coupling #360 removed.
    func saveRideSummary(_ summary: RideSummary)
    func deleteRide(_ id: RideID)

    /// Every ride id this phone has ever downloaded. **Survives `deleteRide`**,
    /// so a deleted ride is never re-counted as "new" (idempotent re-sync, H9).
    func syncedRideIDs() -> Set<RideID>
    func markRideSynced(_ id: RideID)

    /// Ride ids the user deleted *on the phone*. The device keeps its copy
    /// (the SD card is untouched), so the list merge must hide these device
    /// rides instead of resurrecting them on every sync/reload.
    func deletedRideIDs() -> Set<RideID>
    func markRideDeleted(_ id: RideID)
}

/// The no-filesystem conformer: unit tests, previews, and Debug mock runs
/// (persistence across relaunches is `FileLibraryStore`'s job; scenario-driven
/// launches must start from their fixtures alone).
public final class InMemoryLibraryStore: LibraryStore, @unchecked Sendable {
    private let lock = NSLock()
    private var planned: [RouteID: PlannedRouteRecord] = [:]
    private var summaries: [RideID: RideSummary] = [:]
    private var points: [RideID: [RidePoint]] = [:]
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

    public func rideSummaries() -> [RideSummary] {
        lock.withLock { summaries.values.sorted { $0.date > $1.date } }
    }

    public func ridePoints(_ id: RideID) -> [RidePoint]? {
        lock.withLock { points[id] }
    }

    public func saveRide(_ ride: Ride) {
        lock.withLock {
            summaries[ride.id] = ride.summary
            points[ride.id] = ride.points
        }
    }

    public func saveRideSummary(_ summary: RideSummary) {
        lock.withLock { summaries[summary.id] = summary }
    }

    public func deleteRide(_ id: RideID) {
        lock.withLock {
            summaries[id] = nil
            points[id] = nil
        }
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
