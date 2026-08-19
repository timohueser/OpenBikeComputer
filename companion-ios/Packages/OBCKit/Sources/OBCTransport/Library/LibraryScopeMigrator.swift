import Foundation
import OBCDomain

/// The one-time v1 → scoped library migration (#769): **claim-on-first-contact**.
///
/// A v1 library keys everything on bare device object ids — one shared
/// namespace for every device the phone ever synced. Bulk-attributing those
/// flat entries to the currently-bonded serial would mis-key the other
/// device's rides in a mixed (DK + LM20) library and produce exactly the
/// duplicate rows the on-glass checklist forbids. So nothing is attributed by
/// guess: on each connect, flat entries are **claimed** into the connected
/// device's `(serial, epoch, id)` scope only when that device's own ride list
/// **corroborates** them — object id match **and** `start_time` match for ride
/// entries; object-id intersection for the entry-less set ids (synced marks
/// whose files were purged, tombstones). Unclaimed flat entries stay browsable
/// as unscoped archival rides forever.
///
/// **Idempotent and resumable by per-entry ordering, not atomicity.** Each
/// claim writes the scoped twin *first* and removes the flat original *last*
/// (scoped ride → scoped marks → flat marks → flat files), so a kill at any
/// step leaves at worst *both* forms — never neither — and the next contact's
/// re-run converges: a flat entry whose scoped twin already exists is simply
/// swept (the twin is never overwritten — it may carry post-claim edits like a
/// rename). There is no migration journal and no multi-file atomic dance; the
/// disappearance of flat state *is* the completion mark, which is also why
/// `run` cheaply no-ops (`hasLegacyState`) once a library is fully claimed or
/// was born scoped.
///
/// Route links are **not** claimed here (no serial guess — V6's CRC adoption
/// re-links them by content); a flat link already fails the validity predicate
/// by construction, so it can't light a badge or drive a replace-by-id.
public enum LibraryScopeMigrator {
    /// Corroboration tolerance for `start_time`: stored dates round-trip
    /// through JSON `Double` seconds while catalog dates come off a `u32`
    /// wire field — sub-second noise must not defeat an honest match, and
    /// real collisions (a post-reset alias reusing the id) differ by far more.
    private static let startTimeTolerance: TimeInterval = 1

    /// Whether any flat (unscoped) id-keyed state exists — the "is there
    /// anything left to claim" gate callers use to skip the device list read
    /// on every post-migration connect.
    public static func hasLegacyState(in library: any LibraryStore) -> Bool {
        library.rideSummaries().contains { $0.id.scope == nil }
            || library.syncedRideIDs().contains { $0.scope == nil }
            || library.deletedRideIDs().contains { $0.scope == nil }
            || library.trashedRideIDs().keys.contains { $0.scope == nil }
    }

    /// Run one claim pass against the connected device's identity and ride
    /// catalog. `deviceRides` are the catalog summaries as the transport
    /// minted them (scoped ids); only their object ids and start dates are
    /// evidence here. Safe to call on every connect — see the type doc.
    public static func run(
        in library: any LibraryStore,
        scope: LibraryScope,
        deviceRides: [RideSummary]
    ) {
        // Evidence: the connected device's listed object ids, with the start
        // time each id currently carries (first entry wins on a malformed
        // duplicate — ids are unique in a well-formed catalog).
        var listedStart: [UInt64: Date] = [:]
        for ride in deviceRides {
            guard let objectID = ride.id.deviceObjectID else { continue }
            if listedStart[objectID.raw] == nil { listedStart[objectID.raw] = ride.date }
        }
        guard !listedStart.isEmpty else { return }

        let summaries = library.rideSummaries()
        let scopedIDs = Set(summaries.map(\.id).filter { $0.scope != nil })
        let trashed = library.trashedRideIDs()

        // ── 1. Flat ride entries: claim on id + start_time corroboration. ──
        // Bare ids whose entry stays flat (listed id but start_time mismatch —
        // the post-reset alias, or another device's ride): remembered so the
        // set-claim below leaves their marks with them.
        var flatEntryIDsKept: Set<RideID> = []
        for summary in summaries where summary.id.scope == nil {
            guard let objectID = summary.id.deviceObjectID,
                let deviceStart = listedStart[objectID.raw]
            else { continue }
            guard abs(deviceStart.timeIntervalSince(summary.date)) <= startTimeTolerance else {
                flatEntryIDsKept.insert(summary.id)
                continue
            }
            let flatID = summary.id
            let scopedID = RideID(deviceObjectID: objectID, scope: scope)
            // Scoped twin first — but never overwrite one that already exists
            // (an interrupted earlier claim already moved it; it may since
            // have been renamed).
            if !scopedIDs.contains(scopedID) {
                let points = library.ridePoints(flatID) ?? []
                library.saveRide(Ride(summary: summary.rekeyed(to: scopedID), points: points))
            }
            // The entry is possessed, so its scoped id is synced by definition
            // ("downloaded at least once") — this is also what keeps the first
            // post-claim sync from re-downloading it under the new key.
            library.markRideSynced(scopedID)
            // Trash marks follow their rides.
            if let trashedAt = trashed[flatID] {
                library.markRideTrashed(scopedID, at: trashedAt)
                library.unmarkRideTrashed(flatID)
            }
            // Only now retire the flat original (files, then its set mark).
            library.deleteRide(flatID)
            library.unmarkRideSynced(flatID)
        }

        // ── 2. Entry-less flat synced marks: claim by id intersection. ──────
        // A flat mark whose flat *entry* stayed (start_time mismatch) is that
        // entry's mark — claiming it here would stamp "synced" on a ride of
        // this device that never transferred (the incident's exact lie).
        for id in library.syncedRideIDs() where id.scope == nil {
            guard let objectID = id.deviceObjectID, listedStart[objectID.raw] != nil,
                !flatEntryIDsKept.contains(id)
            else { continue }
            library.markRideSynced(RideID(deviceObjectID: objectID, scope: scope))
            library.unmarkRideSynced(id)
        }

        // ── 3. Flat tombstones: claim by id intersection. ───────────────────
        // A tombstone has no entry left to corroborate with; the intersection
        // is the whole evidence (#769's locked call). Mis-attribution across a
        // mixed library resolves in the safe direction on the *other* device:
        // its copy simply resurrects once (the accepted trade-off).
        for id in library.deletedRideIDs() where id.scope == nil {
            guard let objectID = id.deviceObjectID, listedStart[objectID.raw] != nil,
                !flatEntryIDsKept.contains(id)
            else { continue }
            library.markRideDeleted(RideID(deviceObjectID: objectID, scope: scope))
            library.unmarkRideDeleted(id)
        }
    }
}

extension RideSummary {
    /// A copy of this summary under a new id — the claim's re-key write.
    fileprivate func rekeyed(to newID: RideID) -> RideSummary {
        RideSummary(
            id: newID, name: name, date: date,
            distanceMeters: distanceMeters, movingTime: movingTime,
            averageSpeedMps: averageSpeedMps, climbMeters: climbMeters,
            trackPreview: trackPreview,
            avgHeartRate: avgHeartRate, maxHeartRate: maxHeartRate,
            avgCadence: avgCadence, avgPower: avgPower, maxPower: maxPower
        )
    }
}
