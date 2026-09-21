import Foundation
import OBCDomain

/// The phone-side library: planned routes saved from imports and tracked rides synced off the
/// device. It keeps the lists browsable before, without, or away from a device, and it is what
/// makes a re-sync idempotent.
///
/// A seam like `DeviceTransport`: screens and flows see only this protocol, and the composition
/// root picks the conformer. Mock runs use the in-memory one, so every UI-test launch starts from
/// its scenario alone.
///
/// Stores hold canonical domain models, never device wire bytes. Calls are synchronous and
/// expected from the main actor. Ride reads are split: the lists browse the summaries, and a
/// tracklog loads one ride at a time when a detail opens, because a season of rides must never be
/// decoded whole at launch.
public protocol LibraryStore: Sendable {
    // MARK: Planned routes

    /// Every saved planned route, newest first.
    func plannedRoutes() -> [PlannedRouteRecord]
    /// Insert or replace a record under its id: also the write path for renames and for the
    /// uploaded-to-device flip.
    func savePlannedRoute(_ record: PlannedRouteRecord)
    /// Delete a planned route. Also prunes the id from any trip that holds it, because a route
    /// cannot linger as a dangling stage once its record is gone; a trip left with no stages
    /// dissolves.
    func deletePlannedRoute(_ id: RouteID)

    // MARK: Trips

    /// Every saved trip, newest first. A read drops stage ids whose planned-route record is gone,
    /// so a returned trip's stages are always resolvable, and a trip left with no resolvable stage
    /// is dropped.
    func trips() -> [TripRecord]
    /// Insert or replace a trip under its id. Enforces the invariant that a `RouteID` lives in at
    /// most one trip: saving a trip removes its stages from every other stored trip, and any other
    /// trip thereby emptied dissolves. The routes themselves are untouched.
    func saveTrip(_ record: TripRecord)
    /// Delete a trip, with ungroup semantics only. The member route records are untouched and
    /// become top-level; the caller composes a "delete trip and routes" cascade from this and the
    /// per-route deletes.
    func deleteTrip(_ id: TripID)

    // MARK: Tracked rides

    /// Every synced ride's summary, newest first: the Tracked list's whole appetite. Never decodes
    /// tracklogs.
    func rideSummaries() -> [RideSummary]
    /// One ride's full tracklog, loaded on demand for the detail map. Nil when the ride is unknown
    /// or its points do not decode: the ride stays summary-only rather than dropped, and the detail
    /// degrades to the preview's coordinates.
    func ridePoints(_ id: RideID) -> [RidePoint]?
    /// Save one ride's summary and points. Report a write failure before sync records success.
    func saveRide(_ ride: Ride) throws
    /// Commit the downloaded ride and local sync state together. Only a persistent store
    /// can return a receipt; an in-memory preview returns nil.
    func archiveRide(_ ride: Ride) throws -> RideArchiveReceipt?
    /// Source of a complete current archive, distinct from download/deletion history.
    func archivedRideSource(_ id: RideID) -> RideSource?
    /// Revalidate durable storage before returning proof for an existing archive.
    func archivedRideReceipt(_ id: RideID) -> RideArchiveReceipt?
    /// Update a ride's summary without touching its stored points: the rename write path.
    /// Re-encoding a full tracklog to change a name would be exactly the whole-ride coupling the
    /// split read removed.
    func saveRideSummary(_ summary: RideSummary)
    func deleteRide(_ id: RideID)

    /// Current downloaded archives plus explicit local history markers. This is
    /// display/stand-in sync state, never proof for a device source. Explicit history
    /// markers survive `deleteRide`; phone deletion also has its own tombstone.
    func syncedRideIDs() -> Set<RideID>
    func markRideSynced(_ id: RideID)

    /// Ride ids the user deleted on the phone. The device keeps its copy, so the list merge must
    /// hide these device rides instead of resurrecting them on every sync or reload.
    func deletedRideIDs() -> Set<RideID>
    func markRideDeleted(_ id: RideID)

    /// Rides in the phone-side trash, keyed to when each was trashed. A trashed ride keeps its
    /// stored files, and only the Tracked list hides it, so Recover is just clearing the mark. A
    /// permanent delete pairs `deleteRide` with `unmarkRideTrashed`, and the dates drive the
    /// retention purge.
    func trashedRideIDs() -> [RideID: Date]
    func markRideTrashed(_ id: RideID, at date: Date)
    func unmarkRideTrashed(_ id: RideID)
}

/// Confirmation that the local archive's persistence barriers completed.
/// No device receipt is sent by this operation.
public struct RideArchiveReceipt: Equatable, Sendable {
    public let source: RideSource
    init(source: RideSource) { self.source = source }
}

public enum RideArchiveError: Error, Equatable {
    case unsupportedStore
    case invalidSource
    case unreadableArchive
}

extension LibraryStore {
    public func archiveRide(_ ride: Ride) throws -> RideArchiveReceipt? {
        throw RideArchiveError.unsupportedStore
    }
    public func archivedRideSource(_ id: RideID) -> RideSource? { nil }

    public func archivedRideReceipt(_ id: RideID) -> RideArchiveReceipt? { nil }
}

/// The no-filesystem conformer: unit tests, previews, and Debug mock runs. Persistence across
/// relaunches is `FileLibraryStore`'s job, and scenario-driven launches must start from their
/// fixtures alone.
public final class InMemoryLibraryStore: LibraryStore, @unchecked Sendable {
    private let lock = NSLock()
    private var planned: [RouteID: PlannedRouteRecord] = [:]
    private var storedTrips: [TripID: TripRecord] = [:]
    private var summaries: [RideID: RideSummary] = [:]
    private var points: [RideID: [RidePoint]] = [:]
    private var synced: Set<RideID> = []
    private var deleted: Set<RideID> = []
    private var trashed: [RideID: Date] = [:]

    public init() {}

    public func plannedRoutes() -> [PlannedRouteRecord] {
        lock.withLock { planned.values.sorted { $0.addedAt > $1.addedAt } }
    }

    public func savePlannedRoute(_ record: PlannedRouteRecord) {
        lock.withLock { planned[record.id] = record }
    }

    public func deletePlannedRoute(_ id: RouteID) {
        lock.withLock {
            planned[id] = nil
            pruneStageFromTrips(id)
        }
    }

    // MARK: Trips

    public func trips() -> [TripRecord] {
        lock.withLock {
            let alive = Set(planned.keys)
            return storedTrips.values
                .compactMap { trip -> TripRecord? in
                    var trip = trip
                    trip.stageIDs = trip.stageIDs.filter(alive.contains)
                    return trip.stageIDs.isEmpty ? nil : trip
                }
                .sorted { $0.addedAt > $1.addedAt }
        }
    }

    public func saveTrip(_ record: TripRecord) {
        lock.withLock {
            storedTrips[record.id] = record
            // Invariant: a RouteID lives in at most one trip. Strip the saved trip's stages from
            // every other trip; one thereby emptied dissolves.
            let claimed = Set(record.stageIDs)
            for (id, var other) in storedTrips where id != record.id {
                let kept = other.stageIDs.filter { !claimed.contains($0) }
                guard kept.count != other.stageIDs.count else { continue }
                if kept.isEmpty {
                    storedTrips[id] = nil
                } else {
                    other.stageIDs = kept
                    storedTrips[id] = other
                }
            }
        }
    }

    public func deleteTrip(_ id: TripID) {
        lock.withLock { storedTrips[id] = nil }
    }

    /// Remove `route` from every trip that holds it; a trip left with no stages dissolves. The
    /// caller holds `lock`.
    private func pruneStageFromTrips(_ route: RouteID) {
        for (id, var trip) in storedTrips where trip.stageIDs.contains(route) {
            trip.stageIDs.removeAll { $0 == route }
            storedTrips[id] = trip.stageIDs.isEmpty ? nil : trip
        }
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

    public func archiveRide(_ ride: Ride) throws -> RideArchiveReceipt? {
        if let source = ride.summary.source, !source.matches(ride.id) {
            throw RideArchiveError.invalidSource
        }
        lock.withLock {
            summaries[ride.id] = ride.summary
            points[ride.id] = ride.points
            synced.insert(ride.id)
        }
        return nil
    }

    public func archivedRideSource(_ id: RideID) -> RideSource? {
        lock.withLock { points[id] == nil ? nil : summaries[id]?.source }
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

    public func trashedRideIDs() -> [RideID: Date] {
        lock.withLock { trashed }
    }

    public func markRideTrashed(_ id: RideID, at date: Date) {
        lock.withLock { trashed[id] = date }
    }

    public func unmarkRideTrashed(_ id: RideID) {
        lock.withLock { trashed[id] = nil }
    }
}
