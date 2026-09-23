import Foundation
import OBCDomain
import Testing
import OBCTransport

/// A ride's journal lives with the ride: it round-trips, keeps a thumbnail only for a photo it
/// holds, survives a re-archive of the ride, and goes when the ride goes.
struct RideJournalStoreTests {
    enum StoreKind: CaseIterable { case inMemory, file }

    private let id = RideID("ride")

    private func makeStore(_ kind: StoreKind) throws -> LibraryStore {
        let store: LibraryStore
        switch kind {
        case .inMemory:
            store = InMemoryLibraryStore()
        case .file:
            let dir = URL(fileURLWithPath: NSTemporaryDirectory())
                .appendingPathComponent("obc-journal-tests-\(UUID().uuidString)", isDirectory: true)
            store = FileLibraryStore(directory: dir)
        }
        try store.saveRide(ride)
        return store
    }

    private var ride: Ride {
        Ride(
            summary: RideSummary(id: id, name: "Day 2", date: Date(timeIntervalSince1970: 1_000), distanceMeters: 10),
            points: [RidePoint(timestamp: Date(timeIntervalSince1970: 1_000), coordinate: Coordinate(latitude: 46, longitude: 8))]
        )
    }

    private func photo(_ assetID: String, at seconds: Double) -> RidePhoto {
        RidePhoto(assetID: assetID, takenAt: Date(timeIntervalSince1970: seconds))
    }

    @Test(arguments: StoreKind.allCases)
    func theJournalAndItsThumbnailsRoundTrip(_ kind: StoreKind) throws {
        let store = try makeStore(kind)
        var journal = RideJournal()
        journal.add([photo("b/L0/001", at: 2_000), photo("a/L0/001", at: 1_500)])
        journal.close(.photos)

        store.saveRideJournal(journal, thumbnails: ["a/L0/001": Data([1]), "b/L0/001": Data([2])], for: id)

        #expect(store.rideJournal(id) == journal)
        #expect(store.rideJournal(id).photos.map(\.assetID) == ["a/L0/001", "b/L0/001"])
        #expect(store.ridePhotoThumbnails(id) == ["a/L0/001": Data([1]), "b/L0/001": Data([2])])
    }

    @Test(arguments: StoreKind.allCases)
    func aRemovedPhotoTakesItsThumbnail(_ kind: StoreKind) throws {
        let store = try makeStore(kind)
        var journal = RideJournal()
        journal.add([photo("a", at: 1_500), photo("b", at: 2_000)])
        store.saveRideJournal(journal, thumbnails: ["a": Data([1]), "b": Data([2])], for: id)

        journal.remove("a")
        store.saveRideJournal(journal, thumbnails: [:], for: id)

        #expect(store.ridePhotoThumbnails(id) == ["b": Data([2])])
    }

    @Test(arguments: StoreKind.allCases)
    func aRideArchivedAgainKeepsItsJournal(_ kind: StoreKind) throws {
        let store = try makeStore(kind)
        var journal = RideJournal()
        journal.add([photo("a", at: 1_500)])
        store.saveRideJournal(journal, thumbnails: ["a": Data([1])], for: id)

        try store.saveRide(ride)

        #expect(store.rideJournal(id) == journal)
        #expect(store.ridePhotoThumbnails(id) == ["a": Data([1])])
    }

    @Test(arguments: StoreKind.allCases)
    func aDeletedRideTakesItsJournal(_ kind: StoreKind) throws {
        let store = try makeStore(kind)
        var journal = RideJournal()
        journal.add([photo("a", at: 1_500)])
        store.saveRideJournal(journal, thumbnails: ["a": Data([1])], for: id)

        store.deleteRide(id)
        try store.saveRide(ride)

        #expect(store.rideJournal(id) == RideJournal())
        #expect(store.ridePhotoThumbnails(id).isEmpty)
    }

    @Test(arguments: StoreKind.allCases)
    func anUnknownRideKeepsNoJournal(_ kind: StoreKind) throws {
        let store = try makeStore(kind)
        let other = RideID("other")

        store.saveRideJournal(RideJournal(closedRows: [.photos]), thumbnails: [:], for: other)

        #expect(store.rideJournal(other) == RideJournal())
    }
}
