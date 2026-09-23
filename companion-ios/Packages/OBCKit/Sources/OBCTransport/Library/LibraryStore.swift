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
    /// Delete a planned route.
    func deletePlannedRoute(_ id: RouteID)

    // MARK: Trips

    /// Every saved trip, newest first.
    func trips() -> [Trip]
    /// Insert or replace a trip under its id. A trip owns its line, so it touches no route.
    func saveTrip(_ trip: Trip)
    func deleteTrip(_ id: TripID)

    // MARK: Tracked rides

    /// Every synced ride's summary as it was synced, newest first. Never decodes tracklogs. The
    /// rider sees `rideSummaries()`, where edited rides replace the rides they cover.
    func archivedRideSummaries() -> [RideSummary]
    /// One synced ride's full tracklog. Nil when the ride is unknown or its points do not decode.
    func archivedRidePoints(_ id: RideID) -> [RidePoint]?
    /// One ride's line for the all-rides map, edited or not. Nil when its points do not load. A
    /// persistent store caches it, so the map reads a season of rides without decoding a tracklog.
    func rideMapLine(_ id: RideID) -> RideMapLine?
    /// Save one ride's summary and points. Report a write failure before sync records success.
    func saveRide(_ ride: Ride) throws
    /// Commit the downloaded ride and local sync state together. Only a persistent store
    /// can return a receipt; an in-memory preview returns nil.
    func archiveRide(_ ride: Ride) throws -> RideArchiveReceipt?
    /// Source of a complete current archive, distinct from download/deletion history.
    func archivedRideSource(_ id: RideID) -> RideSource?
    /// Revalidate durable storage before returning proof for an existing archive.
    func archivedRideReceipt(_ id: RideID) -> RideArchiveReceipt?
    /// Update a synced ride's summary without touching its stored points: the rename write path.
    /// Re-encoding a full tracklog to change a name would be exactly the whole-ride coupling the
    /// split read removed.
    func saveArchivedRideSummary(_ summary: RideSummary)
    /// Also deletes the ride's journal and photo thumbnails.
    func deleteArchivedRide(_ id: RideID)

    /// The edited rides. `RideViews.swift` reads and edits through these two.
    func rideViews() -> [RideView]
    func saveRideViews(_ views: [RideView])

    /// Merge suggestions the rider dismissed. A dismissed suggestion never comes back.
    func dismissedMerges() -> Set<RidePair>
    func dismissMerge(_ pair: RidePair)

    /// The phone's additions to a synced ride; empty for an unknown ride. The rider sees
    /// `rideJournal(_:)`, where an edited ride shows the photos of its time ranges.
    func archivedRideJournal(_ id: RideID) -> RideJournal
    /// The cached thumbnail of each photo the journal holds, keyed by asset id.
    func archivedRidePhotoThumbnails(_ id: RideID) -> [String: Data]
    /// Writes the journal and the given new thumbnails, and deletes the thumbnails of photos the
    /// journal no longer holds. A ride that is not stored ignores it.
    func saveArchivedRideJournal(_ journal: RideJournal, thumbnails: [String: Data], for id: RideID)

    /// The day note under `key`; empty when there is none.
    func dayNote(_ key: DayNoteKey) -> String
    /// Writes the note; an empty note removes it. A ride's note goes with `deleteRide`; a trip
    /// day's note outlives its rides.
    func saveDayNote(_ note: String, for key: DayNoteKey)

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

    public func rideMapLine(_ id: RideID) -> RideMapLine? {
        ridePoints(id).map { RideMapLine(id: id, points: $0) }
    }
}

/// The no-filesystem conformer: unit tests, previews, and Debug mock runs. Persistence across
/// relaunches is `FileLibraryStore`'s job, and scenario-driven launches must start from their
/// fixtures alone.
public final class InMemoryLibraryStore: LibraryStore, @unchecked Sendable {
    private let lock = NSLock()
    private var planned: [RouteID: PlannedRouteRecord] = [:]
    private var storedTrips: [TripID: Trip] = [:]
    private var summaries: [RideID: RideSummary] = [:]
    private var points: [RideID: [RidePoint]] = [:]
    private var views: [RideView] = []
    private var dismissed: Set<RidePair> = []
    private var synced: Set<RideID> = []
    private var deleted: Set<RideID> = []
    private var trashed: [RideID: Date] = [:]
    private var journals: [RideID: RideJournal] = [:]
    private var thumbnails: [RideID: [String: Data]] = [:]
    private var notes: [DayNoteKey: String] = [:]

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

    // MARK: Trips

    public func trips() -> [Trip] {
        lock.withLock { storedTrips.values.sorted { $0.addedAt > $1.addedAt } }
    }

    public func saveTrip(_ trip: Trip) {
        lock.withLock { storedTrips[trip.id] = trip }
    }

    public func deleteTrip(_ id: TripID) {
        lock.withLock { storedTrips[id] = nil }
    }

    public func archivedRideSummaries() -> [RideSummary] {
        lock.withLock { summaries.values.sorted { $0.date > $1.date } }
    }

    public func archivedRidePoints(_ id: RideID) -> [RidePoint]? {
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

    public func saveArchivedRideSummary(_ summary: RideSummary) {
        lock.withLock { summaries[summary.id] = summary }
    }

    public func deleteArchivedRide(_ id: RideID) {
        lock.withLock {
            summaries[id] = nil
            points[id] = nil
            journals[id] = nil
            thumbnails[id] = nil
            notes[.ride(id)] = nil
        }
    }

    public func dayNote(_ key: DayNoteKey) -> String {
        lock.withLock { notes[key] ?? "" }
    }

    public func saveDayNote(_ note: String, for key: DayNoteKey) {
        lock.withLock { notes[key] = note.isEmpty ? nil : note }
    }

    public func archivedRideJournal(_ id: RideID) -> RideJournal {
        lock.withLock { journals[id] ?? RideJournal() }
    }

    public func archivedRidePhotoThumbnails(_ id: RideID) -> [String: Data] {
        lock.withLock { thumbnails[id] ?? [:] }
    }

    public func saveArchivedRideJournal(_ journal: RideJournal, thumbnails new: [String: Data], for id: RideID) {
        lock.withLock {
            guard summaries[id] != nil else { return }
            journals[id] = journal
            let kept = Set(journal.photos.map(\.assetID))
            thumbnails[id] = (thumbnails[id] ?? [:]).merging(new) { $1 }.filter { kept.contains($0.key) }
        }
    }

    public func rideViews() -> [RideView] {
        lock.withLock { views }
    }

    public func saveRideViews(_ views: [RideView]) {
        lock.withLock { self.views = views }
    }

    public func dismissedMerges() -> Set<RidePair> {
        lock.withLock { dismissed }
    }

    public func dismissMerge(_ pair: RidePair) {
        lock.withLock { _ = dismissed.insert(pair) }
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
