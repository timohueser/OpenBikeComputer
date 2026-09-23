import Testing
import Foundation
import OBCDomain
@testable import OBCTransport

/// Ride edits: trim, split, merge and revert as views over synced rides that never change.
struct RideEditTests {
    /// A ride north at 5 m/s, one point a second, climbing 1 m every 10 s.
    static func ride(
        _ id: String, start: Date, seconds: Int, latitude: Double = 47, trip: RideTrip? = nil
    ) -> Ride {
        let points = (0...seconds).map { second in
            RidePoint(
                timestamp: start.addingTimeInterval(Double(second)),
                coordinate: Coordinate(latitude: latitude + Double(second) * 5 / 111_320, longitude: 8),
                elevationMeters: 500 + Double(second / 10),
                heartRate: 120
            )
        }
        let summary = RideSummary(
            id: RideID(id), name: id, date: start, distanceMeters: 1, movingTime: 1, climbMeters: 1,
            bikeType: .gravel, trip: trip
        )
        return Ride(summary: summary, points: points)
    }

    private let t0 = Date(timeIntervalSince1970: 1_790_000_000)

    private func store(_ rides: Ride...) -> InMemoryLibraryStore {
        let store = InMemoryLibraryStore()
        rides.forEach { store.saveRide($0) }
        return store
    }

    @Test
    func aTrimKeepsTheRangeAndCountsOnlyItsPoints() throws {
        let original = Self.ride("a", start: t0, seconds: 600)
        let store = store(original)
        #expect(store.trimRide(original.id, to: t0.addingTimeInterval(100)...t0.addingTimeInterval(400),
                               summary: original.summary))

        let points = try #require(store.ridePoints(original.id))
        #expect(points.count == 301)
        let trimmed = try #require(store.rideSummaries().first)
        #expect(trimmed.id == original.id)
        #expect(trimmed.name == "a")
        #expect(trimmed.bikeType == .gravel)
        #expect(trimmed.date == t0.addingTimeInterval(100))
        #expect(trimmed.movingTime == 300)
        #expect(abs(trimmed.distanceMeters - 1500) < 5)
        #expect(trimmed.climbMeters == 30)
        #expect(trimmed.avgHeartRate == 120)
        #expect(store.archivedRidePoints(original.id) == original.points, "the synced ride never changes")
    }

    @Test
    func aMergeDoesNotCountTheGapBetweenTheRides() throws {
        let morning = Self.ride("am", start: t0, seconds: 600)
        // Two hours later and 3 km on: neither the time nor the jump counts.
        let afternoon = Self.ride("pm", start: t0.addingTimeInterval(7_800), seconds: 400, latitude: 47.05)
        let store = store(morning, afternoon)
        #expect(store.mergeRides(morning.summary, afternoon.id))

        let rides = store.rideSummaries()
        #expect(rides.map(\.id) == [morning.id], "the merged ride replaces both")
        #expect(rides[0].movingTime == 1_000)
        #expect(abs(rides[0].distanceMeters - 5_000) < 10)
        #expect(store.ridePoints(morning.id)?.count == 601 + 401)
    }

    @Test
    func aSplitPartTakesTheNextFreeNumberAndAMergeGivesTheRideBack() throws {
        let original = Self.ride("Day 2 Ulrichen", start: t0, seconds: 600)
        let lunch = Self.ride("day 2 ulrichen (2)", start: t0.addingTimeInterval(3_600), seconds: 60)
        let store = store(original, lunch)
        let second = try #require(store.splitRide(original.id, at: t0.addingTimeInterval(200),
                                                  summary: original.summary))

        let parts = store.rideSummaries().filter { $0.id != lunch.id }
        #expect(parts.map(\.name) == ["Day 2 Ulrichen (3)", "Day 2 Ulrichen (1)"],
                "\"(2)\" is taken, whatever its case")
        #expect(parts.map(\.id) == [second, original.id])
        #expect(parts.map(\.movingTime) == [400, 200], "the two parts meet at one point, without a gap")

        #expect(store.mergeRides(parts[1], second))
        let view = try #require(store.rideViews().first)
        #expect(view.slices == [RideSlice(source: original.id, start: t0, end: t0.addingTimeInterval(600))])
        #expect(store.rideSummaries().first { $0.id == original.id }?.movingTime == 600)
    }

    @Test
    func revertRestoresEverySyncedRideTheEditsTouchedExactly() throws {
        let a = Self.ride("a", start: t0, seconds: 600)
        let b = Self.ride("b", start: t0.addingTimeInterval(1_000), seconds: 600)
        let c = Self.ride("c", start: t0.addingTimeInterval(5_000), seconds: 600)
        let store = store(a, b, c)
        let synced = store.rideSummaries()
        let half = try #require(store.splitRide(a.id, at: t0.addingTimeInterval(300), summary: a.summary))
        let secondHalf = try #require(store.rideSummaries().first { $0.id == half })
        #expect(store.mergeRides(secondHalf, b.id))
        #expect(store.trimRide(c.id, to: t0.addingTimeInterval(5_100)...t0.addingTimeInterval(5_200),
                               summary: c.summary))

        // Reverting the first half frees `a`, which frees the merge through it, which frees `b`.
        store.revertRide(a.id)
        #expect(store.rideViews().map(\.id) == [c.id], "an edit of an unrelated ride stays")
        #expect(store.rideSummaries().filter { $0.id != c.id } == synced.filter { $0.id != c.id })
        #expect(store.ridePoints(b.id) == b.points)
    }

    @Test
    func deletingAnEditedRideDeletesOnlyTheSyncedRidesNoOtherEditCovers() throws {
        let a = Self.ride("a", start: t0, seconds: 600)
        let b = Self.ride("b", start: t0.addingTimeInterval(1_000), seconds: 600)
        let store = store(a, b)
        let half = try #require(store.splitRide(a.id, at: t0.addingTimeInterval(300), summary: a.summary))
        #expect(store.trimRide(b.id, to: t0.addingTimeInterval(1_000)...t0.addingTimeInterval(1_100),
                               summary: b.summary))

        store.deleteRide(half)
        #expect(store.archivedRidePoints(a.id) != nil, "the first half still needs its synced ride")
        store.deleteRide(b.id)
        #expect(store.archivedRidePoints(b.id) == nil)
        #expect(store.deletedRideIDs() == [b.id], "a sync must not bring the synced ride back")
        #expect(store.rideSummaries().map(\.id) == [a.id])
    }

    @Test
    func aMergeIsSuggestedWhenTheNextRideStartsWhereAndSoonAfterTheFirstEnded() {
        let day2 = RideTrip(key: 7, dayIndex: 1, dayCount: 3, name: "Alps")
        let first = Self.ride("Day 2 Ulrichen", start: t0, seconds: 600, trip: day2)
        let end = first.points.last!.coordinate.latitude
        let lunch = Self.ride("Day 2 Ulrichen (2)", start: t0.addingTimeInterval(3_600), seconds: 600,
                              latitude: end + 0.003, trip: day2)
        #expect(RideEdit.suggestsMerge(first, lunch))
        #expect(!RideEdit.suggestsMerge(lunch, first), "the second ride must start after the first")

        let far = Self.ride("far", start: lunch.summary.date, seconds: 600, latitude: end + 0.006, trip: day2)
        #expect(!RideEdit.suggestsMerge(first, far), "about 670 m away")
        let late = Self.ride("late", start: t0.addingTimeInterval(600 + 12 * 3600 + 1), seconds: 600,
                             latitude: end, trip: day2)
        #expect(!RideEdit.suggestsMerge(first, late))
        var nextDay = lunch
        nextDay.summary.trip?.dayIndex = 2
        #expect(!RideEdit.suggestsMerge(first, nextDay))
        var noTrip = lunch
        noTrip.summary.trip = nil
        #expect(!RideEdit.suggestsMerge(first, noTrip))
    }

    @Test
    func anEditRebuildsTheCachedMapLineAndEditsPersist() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: dir) }
        let ride = Self.ride("a", start: t0, seconds: 600)
        let store = FileLibraryStore(directory: dir)
        try store.saveRide(ride)
        let whole = try #require(store.rideMapLine(ride.id))

        #expect(store.trimRide(ride.id, to: t0...t0.addingTimeInterval(100), summary: ride.summary))
        let reopened = FileLibraryStore(directory: dir)
        let trimmed = try #require(reopened.rideMapLine(ride.id))
        #expect(trimmed != whole)
        #expect(trimmed.pieces.last?.last == ride.points[100].coordinate)
        #expect(reopened.rideSummaries().first?.movingTime == 100)

        reopened.revertRide(ride.id)
        #expect(reopened.rideMapLine(ride.id) == whole)
        #expect(reopened.rideSummaries() == [ride.summary])
    }

    // MARK: Photos

    private func photo(_ name: String, _ second: Double) -> RidePhoto {
        RidePhoto(assetID: name, takenAt: t0.addingTimeInterval(second))
    }

    @Test
    func eachPartOfASplitShowsThePhotosOfItsTimeAndANewPhotoStays() throws {
        let original = Self.ride("a", start: t0, seconds: 3_600)
        let store = store(original)
        store.saveRideJournal(RideJournal(photos: [photo("early", 100), photo("late", 3_000)]), thumbnails: [:], for: original.id)
        let second = try #require(store.splitRide(original.id, at: t0.addingTimeInterval(1_800), summary: original.summary))

        #expect(store.rideJournal(original.id).photos.map(\.assetID) == ["early"])
        #expect(store.rideJournal(second).photos.map(\.assetID) == ["late"])

        var journal = store.rideJournal(second)
        journal.add([photo("new", 2_500)])
        store.saveRideJournal(journal, thumbnails: ["new": Data([1])], for: second)
        #expect(store.rideJournal(second).photos.map(\.assetID) == ["new", "late"])
        #expect(store.ridePhotoThumbnails(second) == ["new": Data([1])])
        #expect(store.rideJournal(original.id).photos.map(\.assetID) == ["early"], "part 1 does not change")
    }

    @Test
    func aMergeShowsThePhotosOfBothRides() throws {
        let morning = Self.ride("am", start: t0, seconds: 600)
        let afternoon = Self.ride("pm", start: t0.addingTimeInterval(7_800), seconds: 400)
        let store = store(morning, afternoon)
        store.saveRideJournal(RideJournal(photos: [photo("am", 100)]), thumbnails: [:], for: morning.id)
        store.saveRideJournal(RideJournal(photos: [photo("pm", 7_900)]), thumbnails: [:], for: afternoon.id)
        #expect(store.mergeRides(morning.summary, afternoon.id))
        #expect(store.rideJournal(morning.id).photos.map(\.assetID) == ["am", "pm"])
    }

    @Test
    func aTrimHidesThePhotosItCutsAndRevertShowsThemAgain() throws {
        let original = Self.ride("a", start: t0, seconds: 3_600)
        let store = store(original)
        let photos = [photo("kept", 100), photo("cut", 3_000)]
        store.saveRideJournal(RideJournal(photos: photos), thumbnails: [:], for: original.id)
        #expect(store.trimRide(original.id, to: t0...t0.addingTimeInterval(600), summary: original.summary))
        #expect(store.rideJournal(original.id).photos.map(\.assetID) == ["kept"])

        // A save on the trimmed ride keeps the hidden photo with the synced ride.
        store.saveRideJournal(store.rideJournal(original.id), thumbnails: [:], for: original.id)
        store.revertRide(original.id)
        #expect(store.rideJournal(original.id).photos == photos)
    }
}
