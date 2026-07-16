import Foundation
import Testing
import OBCDomain
import OBCTransport

/// The one-time v1 → scoped migration (#769): claim-on-first-contact. The
/// mixed two-device library is the case the whole design bends around — flat
/// entries claim only where the device's own ride list corroborates them
/// (id + start_time), so nothing is attributed by guess and NO duplicate rows
/// appear; and every claim is per-entry idempotent, so an app kill anywhere
/// in the pass converges on the next contact.
@Suite struct LibraryScopeMigratorTests {
    private let deviceA = LibraryScope(serial: "OBC-DK-000001", epoch: 0xAAAA_0001)
    private let deviceB = LibraryScope(serial: "OBC-24-000317", epoch: 0xBBBB_0001)

    private let startA3 = Date(timeIntervalSince1970: 1_700_000_000)
    private let startB3 = Date(timeIntervalSince1970: 1_710_000_000)

    private func flatRide(_ objectID: UInt16, name: String, date: Date,
                          points: [RidePoint] = []) -> Ride {
        Ride(summary: RideSummary(id: RideID(deviceObjectID: DeviceObjectID(objectID)),
                                  name: name, date: date, distanceMeters: 10_000),
             points: points)
    }

    /// A device catalog entry as the transport would mint it — scoped id,
    /// wire-truth start date.
    private func listed(_ objectID: UInt16, date: Date, scope: LibraryScope) -> RideSummary {
        RideSummary(id: RideID(deviceObjectID: DeviceObjectID(objectID), scope: scope),
                    name: "on-device", date: date, distanceMeters: 10_000)
    }

    // MARK: The basic claim

    @Test func corroboratedFlatRideClaimsIntoTheScope() {
        let library = InMemoryLibraryStore()
        let point = RidePoint(timestamp: startA3, coordinate: Coordinate(latitude: 48, longitude: 8))
        library.saveRide(flatRide(3, name: "Morning Loop", date: startA3, points: [point]))
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(3)))

        LibraryScopeMigrator.run(in: library, scope: deviceA,
                                 deviceRides: [listed(3, date: startA3, scope: deviceA)])

        let scopedID = RideID(deviceObjectID: DeviceObjectID(3), scope: deviceA)
        #expect(library.rideSummaries().map(\.id) == [scopedID], "one row, re-keyed — never two")
        #expect(library.rideSummaries().first?.name == "Morning Loop")
        #expect(library.ridePoints(scopedID) == [point], "the tracklog moves with the claim")
        #expect(library.syncedRideIDs() == [scopedID], "the flat mark is consumed, the scoped one minted")
    }

    /// id match alone is NOT corroboration: a start_time mismatch is the
    /// post-reset alias (or the other device's ride) — the entry stays flat,
    /// browsable as an archival row forever.
    @Test func startTimeMismatchLeavesTheEntryFlat() {
        let library = InMemoryLibraryStore()
        library.saveRide(flatRide(3, name: "Someone else's ride 3", date: startB3))
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(3)))

        LibraryScopeMigrator.run(in: library, scope: deviceA,
                                 deviceRides: [listed(3, date: startA3, scope: deviceA)])

        let flatID = RideID(deviceObjectID: DeviceObjectID(3))
        #expect(library.rideSummaries().map(\.id) == [flatID], "unclaimed — archival, not lost")
        #expect(library.syncedRideIDs() == [flatID],
                "its synced mark stays with it — claiming the mark alone would stamp 'synced' on a device ride that never transferred")
        #expect(!library.syncedRideIDs().contains(
            RideID(deviceObjectID: DeviceObjectID(3), scope: deviceA)))
    }

    @Test func unlistedFlatEntriesAreUntouched() {
        let library = InMemoryLibraryStore()
        library.saveRide(flatRide(50, name: "From a device long gone", date: startA3))
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(50)))

        LibraryScopeMigrator.run(in: library, scope: deviceA,
                                 deviceRides: [listed(3, date: startA3, scope: deviceA)])

        #expect(library.rideSummaries().map(\.id) == [RideID(deviceObjectID: DeviceObjectID(50))])
        #expect(library.syncedRideIDs() == [RideID(deviceObjectID: DeviceObjectID(50))])
    }

    // MARK: The mixed two-device library (the on-glass checklist's forbidden duplicate)

    /// A v1 library holding BOTH devices' "ride 3" (today's shared namespace
    /// collision means only one entry survived — the last synced one, device
    /// B's). First contact with A: B's entry doesn't corroborate (start_time)
    /// and stays put. First contact with B: it claims. **No duplicate rows at
    /// any point**, and A's ride 3 is free to sync as a new row later.
    @Test func mixedTwoDeviceLibraryClaimsWithoutDuplicates() {
        let library = InMemoryLibraryStore()
        library.saveRide(flatRide(3, name: "B's ride 3", date: startB3))
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(3)))

        // Contact with device A (also lists a ride 3 — its own).
        LibraryScopeMigrator.run(in: library, scope: deviceA,
                                 deviceRides: [listed(3, date: startA3, scope: deviceA)])
        #expect(library.rideSummaries().count == 1, "no duplicate after A's contact")
        #expect(library.rideSummaries().first?.id.scope == nil, "B's entry did not claim to A")

        // Contact with device B.
        LibraryScopeMigrator.run(in: library, scope: deviceB,
                                 deviceRides: [listed(3, date: startB3, scope: deviceB)])
        let scopedB = RideID(deviceObjectID: DeviceObjectID(3), scope: deviceB)
        #expect(library.rideSummaries().map(\.id) == [scopedB], "exactly one row, B's key")
        #expect(library.syncedRideIDs() == [scopedB])

        // Re-contact with A changes nothing further (idempotent, no cross-talk).
        LibraryScopeMigrator.run(in: library, scope: deviceA,
                                 deviceRides: [listed(3, date: startA3, scope: deviceA)])
        #expect(library.rideSummaries().map(\.id) == [scopedB])
    }

    // MARK: Interruption / resume (per-entry idempotence)

    /// The claim's kill window: the scoped twin was written but the flat
    /// original not yet removed (write-new-first, delete-old-last — the order
    /// that can lose nothing). The re-run must sweep the flat leftover and
    /// must NOT overwrite the scoped twin — the user may have renamed it
    /// since.
    @Test func rerunAfterATornClaimSweepsTheFlatLeftoverWithoutClobbering() {
        let library = InMemoryLibraryStore()
        library.saveRide(flatRide(3, name: "Morning Loop", date: startA3))
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(3)))
        // Simulate the interrupted first pass: scoped twin exists (already
        // renamed by the user), flat entry + flat mark still present.
        let scopedID = RideID(deviceObjectID: DeviceObjectID(3), scope: deviceA)
        library.saveRide(Ride(
            summary: RideSummary(id: scopedID, name: "Renamed after claim", date: startA3,
                                 distanceMeters: 10_000),
            points: []))
        library.markRideSynced(scopedID)

        LibraryScopeMigrator.run(in: library, scope: deviceA,
                                 deviceRides: [listed(3, date: startA3, scope: deviceA)])

        #expect(library.rideSummaries().map(\.id) == [scopedID])
        #expect(library.rideSummaries().first?.name == "Renamed after claim",
                "the re-run never overwrites the scoped twin")
        #expect(library.syncedRideIDs() == [scopedID])
    }

    @Test func runningTheClaimTwiceIsANoOp() {
        let library = InMemoryLibraryStore()
        library.saveRide(flatRide(3, name: "Morning Loop", date: startA3))
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(3)))

        let catalog = [listed(3, date: startA3, scope: deviceA)]
        LibraryScopeMigrator.run(in: library, scope: deviceA, deviceRides: catalog)
        let after1 = (library.rideSummaries().map(\.id), library.syncedRideIDs())
        LibraryScopeMigrator.run(in: library, scope: deviceA, deviceRides: catalog)
        #expect(library.rideSummaries().map(\.id) == after1.0)
        #expect(library.syncedRideIDs() == after1.1)
        #expect(!LibraryScopeMigrator.hasLegacyState(in: library),
                "a fully-claimed library reports no legacy state — the per-connect gate closes")
    }

    // MARK: Trash marks follow their rides

    @Test func trashMarkFollowsAClaimedRide() {
        let library = InMemoryLibraryStore()
        let flatID = RideID(deviceObjectID: DeviceObjectID(3))
        let trashedAt = Date(timeIntervalSince1970: 1_705_000_000)
        library.saveRide(flatRide(3, name: "Trashed one", date: startA3))
        library.markRideSynced(flatID)
        library.markRideTrashed(flatID, at: trashedAt)

        LibraryScopeMigrator.run(in: library, scope: deviceA,
                                 deviceRides: [listed(3, date: startA3, scope: deviceA)])

        let scopedID = RideID(deviceObjectID: DeviceObjectID(3), scope: deviceA)
        #expect(library.trashedRideIDs() == [scopedID: trashedAt])
    }

    // MARK: Entry-less set ids (purged synced marks, tombstones)

    /// A synced mark whose files were long deleted claims by plain id
    /// intersection — there is no start_time left to check (#769's locked
    /// call for set ids).
    @Test func entryLessSyncedMarkClaimsByIntersection() {
        let library = InMemoryLibraryStore()
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(3)))
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(99)))  // not listed

        LibraryScopeMigrator.run(in: library, scope: deviceA,
                                 deviceRides: [listed(3, date: startA3, scope: deviceA)])

        #expect(library.syncedRideIDs() == [
            RideID(deviceObjectID: DeviceObjectID(3), scope: deviceA),
            RideID(deviceObjectID: DeviceObjectID(99)),
        ])
    }

    @Test func tombstoneClaimsByIntersection() {
        let library = InMemoryLibraryStore()
        // The v1 permanent delete left both marks (synced + deleted).
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(4)))
        library.markRideDeleted(RideID(deviceObjectID: DeviceObjectID(4)))

        LibraryScopeMigrator.run(in: library, scope: deviceA,
                                 deviceRides: [listed(4, date: startA3, scope: deviceA)])

        let scoped = RideID(deviceObjectID: DeviceObjectID(4), scope: deviceA)
        #expect(library.syncedRideIDs() == [scoped])
        #expect(library.deletedRideIDs() == [scoped],
                "the device's copy stays hidden and un-resyncable under the claimed tombstone")
    }

    // MARK: On disk (FileLibraryStore) — scoped keys are just file names

    /// The claim through the real file store: the flat `rides/3/` directory
    /// becomes a scoped one (the id string is the directory name — scoping
    /// needs no layout of its own), the set files carry the scoped strings,
    /// and a *relaunch* (a second store over the same directory) reads it all
    /// back. Run twice to pin idempotence against real files.
    @Test func claimRekeysOnDiskAndSurvivesARelaunch() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("obc-migrator-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = FileLibraryStore(directory: dir)

        let point = RidePoint(timestamp: startA3, coordinate: Coordinate(latitude: 48, longitude: 8))
        store.saveRide(flatRide(3, name: "Morning Loop", date: startA3, points: [point]))
        store.markRideSynced(RideID(deviceObjectID: DeviceObjectID(3)))
        store.markRideTrashed(RideID(deviceObjectID: DeviceObjectID(3)), at: startA3)

        let catalog = [listed(3, date: startA3, scope: deviceA)]
        LibraryScopeMigrator.run(in: store, scope: deviceA, deviceRides: catalog)
        LibraryScopeMigrator.run(in: store, scope: deviceA, deviceRides: catalog)  // idempotent

        let relaunched = FileLibraryStore(directory: dir)
        let scopedID = RideID(deviceObjectID: DeviceObjectID(3), scope: deviceA)
        #expect(relaunched.rideSummaries().map(\.id) == [scopedID])
        #expect(relaunched.ridePoints(scopedID) == [point])
        #expect(relaunched.syncedRideIDs() == [scopedID])
        #expect(relaunched.trashedRideIDs() == [scopedID: startA3])
        #expect(!LibraryScopeMigrator.hasLegacyState(in: relaunched))
    }

    /// An empty catalog (fresh device, or a list read that yielded nothing)
    /// claims nothing — corroboration requires listed evidence.
    @Test func emptyCatalogClaimsNothing() {
        let library = InMemoryLibraryStore()
        library.saveRide(flatRide(3, name: "Keep me flat", date: startA3))
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(3)))

        LibraryScopeMigrator.run(in: library, scope: deviceA, deviceRides: [])

        #expect(library.rideSummaries().first?.id.scope == nil)
        #expect(LibraryScopeMigrator.hasLegacyState(in: library))
    }
}
