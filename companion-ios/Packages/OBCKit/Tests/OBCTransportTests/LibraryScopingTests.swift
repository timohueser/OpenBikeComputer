import Foundation
import Testing
import OBCDomain
import OBCTransport

/// Store identity is part of each current device key; old keys stay archival.
@Suite struct LibraryScopingTests {
    private let deviceA = LibraryScope(serial: "OBC-24-000317", storeID: "1111111111111111111111110bc00001")

    // MARK: The id encoding

    @Test func scopedIDRoundTripsItsParts() {
        let id = RideID(deviceObjectID: DeviceObjectID(42), scope: deviceA)
        #expect(id.rawValue == "v4:1111111111111111111111110bc00001:42:OBC-24-000317")
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

    /// The serial rides last, so a serial containing the separator needs no escaping and the
    /// encoding stays injective.
    @Test func serialContainingColonsRoundTrips() {
        let odd = LibraryScope(serial: "OBC:rev:B:00 17", storeID: "00000000000000000000000000000007")
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

    @Test(arguments: ["v2:1:3:S", "v4:short:3:S",
                      "v4:1111111111111111111111110bc00001:notanumber:S",
                      "v4:1111111111111111111111110bc00001:18446744073709551616:S"])
    func malformedScopedIDsReadAsUnscoped(raw: String) {
        let id = RideID(raw)
        #expect(id.scope == nil)
        #expect(id.rawValue == raw)
    }

    // MARK: The era matrix (key validity)

    /// Device wiped, app kept: the same object ids come back under a fresh StoreId. Every old key
    /// stops matching, and the old entries stay browsable under their old keys.
    @Test func deviceWipedAppKept() {
        let oldEra = deviceA
        let newEra = LibraryScope(serial: deviceA.serial, storeID: "2222222222222222222222220bc00001")

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

    /// App reinstalled, device kept: identity comes from the device, so rides land under the
    /// exact same keys the lost library used.
    @Test func appReinstallDeviceKept() {
        let mintedBeforeReinstall = RideID(deviceObjectID: DeviceObjectID(7), scope: deviceA)
        let mintedAfterReinstall = RideID(deviceObjectID: DeviceObjectID(7), scope: deviceA)
        #expect(mintedBeforeReinstall == mintedAfterReinstall)
    }

    /// The same object id on two devices is two distinct keys: no shared rows, and one device's
    /// tombstones say nothing about the other.
    @Test func serialSwitchHasNoCrossTalk() {
        let dk = LibraryScope(serial: "OBC-DK-000001", storeID: "00000000000000000000000000000001")
        let lm20 = LibraryScope(serial: "OBC-24-000317", storeID: "00000000000000000000000000000001")

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

    // MARK: The route-link validity predicate

    @Test func linkMatchesOnlyItsOwnScope() {
        let link = DeviceRouteLink(serial: deviceA.serial, storeID: deviceA.storeID,
                                   objectID: DeviceObjectID(5))
        #expect(link.matches(deviceA))
        #expect(!link.matches(LibraryScope(serial: deviceA.serial, storeID: "2222222222222222222222220bc00001")),
                "an era change invalidates the link")
        #expect(!link.matches(LibraryScope(serial: "OBC-DK-000001", storeID: deviceA.storeID)),
                "another device never matches")
    }

    // MARK: The scope's fail-closed inputs

    @Test func deviceInfoWithoutStoreIDYieldsNoScope() {
        let info = DeviceInfo(name: "OBC", firmwareVersion: "1.0", serial: "OBC-24-000317",
                              storeID: nil)
        #expect(info.libraryScope == nil)
    }

    @Test func deviceInfoWithEmptySerialYieldsNoScope() {
        let info = DeviceInfo(name: "OBC", firmwareVersion: "1.0", serial: "", storeID: "00000000000000000000000000000007")
        #expect(info.libraryScope == nil)
    }

    @Test(arguments: ["", "1234", "0000000000000000000000000000000g",
                      "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"])
    func malformedStoreIDYieldsNoScope(storeID: String) {
        let info = DeviceInfo(name: "OBC", firmwareVersion: "1.0", serial: deviceA.serial,
                              storeID: storeID)
        #expect(info.libraryScope == nil)
    }

    // MARK: Helpers

    private func summary(id: RideID, name: String) -> RideSummary {
        RideSummary(id: id, name: name, date: Date(timeIntervalSince1970: 1_700_000_000),
                    distanceMeters: 10_000)
    }
}
