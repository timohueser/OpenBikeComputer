import Foundation
import Testing
import OBCDomain
import OBCTransport

/// The (serial, epoch) composite keys (#769): the id encoding itself, and the
/// **era matrix as key-validity tests** — the whole point of scope-in-the-key
/// is that a device wipe, an app reinstall, a device switch, or a torn-marks
/// epoch mint needs *zero* migration code, because the outcome is decided by
/// whether strings match, not by any mutate-on-mismatch flow.
@Suite struct LibraryScopingTests {
    private let deviceA = LibraryScope(serial: "OBC-24-000317", epoch: 0x1111_1111)

    // MARK: The id encoding

    @Test func scopedIDRoundTripsItsParts() {
        let id = RideID(deviceObjectID: DeviceObjectID(42), scope: deviceA)
        #expect(id.rawValue == "v2:286331153:42:OBC-24-000317")
        #expect(id.scope == deviceA)
        #expect(id.deviceObjectID == DeviceObjectID(42))
    }

    @Test func flatStoreIDRoundTripsWithoutCollapsingToAnEpoch() {
        let storeID = "8f2c41d96b074ea3b1559c207de83466"
        let scope = LibraryScope(serial: "OBC-24-000317", storeID: storeID)
        let id = RideID(deviceObjectID: DeviceObjectID(0x1_0000), scope: scope)
        #expect(id.rawValue == "v4:\(storeID):65536:OBC-24-000317")
        #expect(id.scope == scope)
        #expect(id.deviceObjectID == DeviceObjectID(0x1_0000))
        #expect(DeviceRouteLink(scope: scope, objectID: DeviceObjectID(7)).matches(scope))
    }

    /// The serial rides last so a serial containing the separator needs no
    /// escaping — the encoding stays injective.
    @Test func serialContainingColonsRoundTrips() {
        let odd = LibraryScope(serial: "OBC:rev:B:00 17", epoch: 7)
        let id = RideID(deviceObjectID: DeviceObjectID(3), scope: odd)
        #expect(id.scope == odd)
        #expect(id.deviceObjectID == DeviceObjectID(3))
    }

    @Test func legacyFlatIDParsesItsObjectIDButHasNoScope() {
        let flat = RideID(deviceObjectID: DeviceObjectID(9))
        #expect(flat.rawValue == "9")
        #expect(flat.scope == nil)
        #expect(flat.deviceObjectID == DeviceObjectID(9))
    }

    @Test func fixtureStyleStringIDsAreNeitherScopedNorDeviceIDs() {
        let id = RideID("ride-kettle-moraine")
        #expect(id.scope == nil)
        #expect(id.deviceObjectID == nil)
    }

    /// A malformed `v2:` prefix (non-numeric epoch/object id) is not silently
    /// half-parsed — it reads as an opaque unscoped id.
    @Test(arguments: ["v2:notanumber:3:S", "v2:1:notanumber:S", "v2:1:99999:S", "v2:1:3"])
    func malformedScopedIDsReadAsUnscoped(raw: String) {
        let id = RideID(raw)
        #expect(id.scope == nil)
    }

    // MARK: The era matrix (key validity)

    /// Device wiped, app kept: the same object ids come back under a fresh
    /// epoch — every old key stops matching (no suppression of the new era's
    /// rides, no resurrection *into* the new era's sets), and the old entries
    /// stay browsable under their old keys.
    @Test func deviceWipedAppKept() {
        let oldEra = deviceA
        let newEra = LibraryScope(serial: deviceA.serial, epoch: 0x2222_2222)

        let library = InMemoryLibraryStore()
        let oldID = RideID(deviceObjectID: DeviceObjectID(3), scope: oldEra)
        library.saveRide(Ride(summary: summary(id: oldID, name: "Old-era ride"), points: []))
        library.markRideSynced(oldID)
        library.markRideDeleted(RideID(deviceObjectID: DeviceObjectID(4), scope: oldEra))

        // The device returns wiped: ride 3 exists again, as a different ride.
        let newID = RideID(deviceObjectID: DeviceObjectID(3), scope: newEra)
        #expect(newID != oldID, "same serial + object id, different era → different key")
        #expect(!library.syncedRideIDs().contains(newID),
                "the new era's ride 3 is NOT 'already synced'")
        #expect(!library.deletedRideIDs().contains(RideID(deviceObjectID: DeviceObjectID(4), scope: newEra)),
                "the new era's ride 4 is NOT tombstoned — the old delete belonged to the old era")
        // The old-era entry is archival, not lost.
        #expect(library.rideSummaries().map(\.id) == [oldID])
    }

    /// App reinstalled, device kept: rides land under the exact same
    /// (serial, epoch, id) keys the lost library used — identity is derived
    /// from the device, so nothing app-local is needed to reproduce it.
    @Test func appReinstallDeviceKept() {
        let mintedBeforeReinstall = RideID(deviceObjectID: DeviceObjectID(7), scope: deviceA)
        let mintedAfterReinstall = RideID(deviceObjectID: DeviceObjectID(7), scope: deviceA)
        #expect(mintedBeforeReinstall == mintedAfterReinstall)
    }

    /// Serial switch (the DK ↔ LM20 pair): the same object id on two devices
    /// is two distinct keys — no shared rows, no cross-device suppression,
    /// device B's tombstones say nothing about device A.
    @Test func serialSwitchHasNoCrossTalk() {
        let dk = LibraryScope(serial: "OBC-DK-000001", epoch: 1)
        let lm20 = LibraryScope(serial: "OBC-24-000317", epoch: 1)

        let library = InMemoryLibraryStore()
        let dkRide = RideID(deviceObjectID: DeviceObjectID(3), scope: dk)
        let lmRide = RideID(deviceObjectID: DeviceObjectID(3), scope: lm20)
        library.saveRide(Ride(summary: summary(id: dkRide, name: "DK ride 3"), points: []))
        library.saveRide(Ride(summary: summary(id: lmRide, name: "LM20 ride 3"), points: []))
        library.markRideSynced(dkRide)
        library.markRideDeleted(lmRide)

        #expect(Set(library.rideSummaries().map(\.id)) == [dkRide, lmRide],
                "both devices' ride 3 are distinct library rows")
        #expect(!library.syncedRideIDs().contains(lmRide))
        #expect(!library.deletedRideIDs().contains(dkRide))
    }

    /// A torn id-marks write mints a fresh epoch (the firmware's mint rule,
    /// V3): from the app's side that is indistinguishable from any other era
    /// change — new scope, empty sets, old keys archival. Same assertion
    /// shape as the wipe, pinned separately because the *device state* differs
    /// (the card kept its rides; only RRAM tore).
    @Test func tornMarksMintedEpochIsANewScope() {
        let before = deviceA
        let after = LibraryScope(serial: deviceA.serial, epoch: 0x3333_3333)
        let library = InMemoryLibraryStore()
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(12), scope: before))

        // The card survived, so ride 12 still exists on the device — under the
        // new epoch it re-syncs once (resurrection is the accepted, safe
        // direction) because the old synced mark no longer matches.
        #expect(!library.syncedRideIDs().contains(
            RideID(deviceObjectID: DeviceObjectID(12), scope: after)))
    }

    // MARK: The route-link validity predicate

    @Test func linkMatchesOnlyItsOwnScope() {
        let link = DeviceRouteLink(serial: deviceA.serial, epoch: deviceA.epoch,
                                   objectID: DeviceObjectID(5))
        #expect(link.matches(deviceA))
        #expect(!link.matches(LibraryScope(serial: deviceA.serial, epoch: 999)),
                "an era change invalidates the link")
        #expect(!link.matches(LibraryScope(serial: "OBC-DK-000001", epoch: deviceA.epoch)),
                "another device never matches")
    }

    // MARK: The scope's fail-closed inputs

    @Test func deviceInfoWithoutEpochYieldsNoScope() {
        let info = DeviceInfo(name: "OBC", firmwareVersion: "1.0", serial: "OBC-24-000317",
                              storeEpoch: nil)
        #expect(info.libraryScope == nil)
    }

    @Test func deviceInfoWithEmptySerialYieldsNoScope() {
        let info = DeviceInfo(name: "OBC", firmwareVersion: "1.0", serial: "", storeEpoch: 7)
        #expect(info.libraryScope == nil)
    }

    /// `0` is a legal epoch — a device whose TRNG minted zero must scope
    /// normally, which is exactly why a missing epoch is `nil`, never `0`.
    @Test func zeroIsALegalEpoch() {
        let info = DeviceInfo(name: "OBC", firmwareVersion: "1.0", serial: "OBC-24-000317",
                              storeEpoch: 0)
        #expect(info.libraryScope == LibraryScope(serial: "OBC-24-000317", epoch: 0))
    }

    // MARK: Helpers

    private func summary(id: RideID, name: String) -> RideSummary {
        RideSummary(id: id, name: name, date: Date(timeIntervalSince1970: 1_700_000_000),
                    distanceMeters: 10_000)
    }
}
