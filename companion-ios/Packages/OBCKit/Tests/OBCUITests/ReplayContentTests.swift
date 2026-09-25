import Foundation
import OBCDomain
import OBCTransport
import Testing
@testable import OBCUI

struct ReplayContentTests {
    private func point(_ east: Double, at second: Double, height: Double? = nil,
                       start: Bool = false) -> RidePoint {
        RidePoint(
            timestamp: Date(timeIntervalSince1970: second),
            coordinate: Coordinate(latitude: 46, longitude: 8 + east / 77_170),
            elevationMeters: height, segmentStart: start)
    }

    @Test func recordedGapsAndBadFixesDoNotBecomeDistance() throws {
        let invalid = RidePoint(timestamp: Date(timeIntervalSince1970: 4),
                                coordinate: Coordinate(latitude: .nan, longitude: 8))
        let points = [
            point(0, at: 0), point(0, at: 1), point(100, at: 2, height: .infinity),
            point(10_000, at: 3, start: true), invalid,
            point(20_000, at: 5), point(20_100, at: 6),
        ]
        let content = try #require(ReplayContent.ride(title: "Ride", ride: .init(points: points)))
        #expect(content.points.count == 6)
        #expect(content.points.map(\.segmentStart) == [true, false, false, true, true, false])
        #expect(content.points[1].distance == 0)
        #expect(content.points[3].distance == content.points[2].distance)
        #expect(content.points[4].distance == content.points[3].distance)
        #expect(content.points[2].elevation == nil)
        #expect(content.totalDistance > 180 && content.totalDistance < 220)
        #expect(content.durationSeconds == 60)
    }

    @Test func photoMomentUsesTimeOnTheCorrectOutAndBackLeg() throws {
        let points = [point(0, at: 0), point(100, at: 10), point(0, at: 20)]
        let photo = RidePhoto(assetID: "return", takenAt: Date(timeIntervalSince1970: 15))
        let image = Data([1, 2, 3])
        let content = try #require(ReplayContent.ride(
            title: "Out and back", ride: .init(points: points, photos: [photo], thumbnails: ["return": image])))
        let moment = try #require(content.photos.first)
        #expect(moment.id == "return")
        #expect(moment.distance > content.totalDistance / 2)
        #expect(moment.distance < content.totalDistance)
        #expect(moment.thumbnailData == image)
    }

    @Test func datelineCrossingKeepsTrackAndPhotoOnTheShortDistanceAxis() throws {
        let a = RidePoint(timestamp: Date(timeIntervalSince1970: 0),
                          coordinate: Coordinate(latitude: 0, longitude: 179.999))
        let b = RidePoint(timestamp: Date(timeIntervalSince1970: 10),
                          coordinate: Coordinate(latitude: 0, longitude: -179.999))
        let photo = RidePhoto(assetID: "dateline", takenAt: Date(timeIntervalSince1970: 5))
        let content = try #require(ReplayContent.ride(
            title: "Crossing", ride: .init(points: [a, b], photos: [photo])))
        #expect(content.totalDistance > 200 && content.totalDistance < 250)
        #expect(abs(content.points[1].distance - content.totalDistance) < 0.01)
        #expect(abs((content.photos.first?.distance ?? 0) - content.totalDistance / 2) < 1)
    }

    @Test func tripIncludesOnlyRiddenDaysAndStartsEachRideAfterAGap() throws {
        let first = ReplayContent.Ride(points: [point(0, at: 0), point(100, at: 10)])
        let second = ReplayContent.Ride(points: [point(10_000, at: 20), point(10_100, at: 30)])
        let content = try #require(ReplayContent.trip(title: "Trip", days: [
            (name: "Day 1", rides: [first]),
            (name: "Day 2", rides: [second]),
            (name: "Day 3", rides: []),
        ]))
        #expect(content.days.map(\.name) == ["Day 1", "Day 2"])
        #expect(content.points.map(\.segmentStart) == [true, false, true, false])
        #expect(content.points[2].distance == content.points[1].distance)
        #expect(content.days[1].distance == content.points[2].distance)
        #expect(content.totalDistance > 180 && content.totalDistance < 220)
        #expect(content.durationSeconds == 120)
    }

    @Test func tripPhotoIDsStayUniqueAcrossRides() throws {
        let photo = RidePhoto(assetID: "shared", takenAt: Date(timeIntervalSince1970: 10))
        let first = ReplayContent.Ride(
            points: [point(0, at: 0), point(100, at: 10)], photos: [photo])
        let second = ReplayContent.Ride(
            points: [point(10_000, at: 10), point(10_100, at: 20)], photos: [photo])
        let content = try #require(ReplayContent.trip(title: "Trip", days: [
            (name: "Day 1", rides: [first]), (name: "Day 2", rides: [second]),
        ]))
        #expect(content.photos.map(\.id) == ["shared"])
    }

    @Test func emptyAndStationaryTracksHaveNoReplay() {
        #expect(ReplayContent.ride(title: "Empty", ride: .init(points: [])) == nil)
        #expect(ReplayContent.ride(title: "Stopped", ride: .init(points: [point(0, at: 0), point(0, at: 1)])) == nil)
    }

    @MainActor @Test func journalReplayExcludesFuturePlannedDays() async throws {
        let library = InMemoryLibraryStore()
        let day1 = [point(0, at: 0).coordinate, point(100, at: 10).coordinate]
            .map { RoutePoint(coordinate: $0) }
        let day2 = [point(10_000, at: 20).coordinate, point(10_100, at: 30).coordinate]
            .map { RoutePoint(coordinate: $0) }
        let trip = Trip.joining([day1, day2], id: TripID("replay-trip"),
                                name: "Two days", bikeType: .touring,
                                now: Date(timeIntervalSince1970: 0))
        let summary = RideSummary(
            id: RideID("first"), name: "First day", date: Date(timeIntervalSince1970: 0),
            distanceMeters: 100,
            trip: RideTrip(key: trip.key, dayIndex: 0, dayCount: trip.dayCount, name: trip.name))
        try library.saveRide(Ride(summary: summary, points: [point(0, at: 0), point(100, at: 10)]))

        let journal = TripJournalModel(library: library)
        await journal.load(trip: trip, rides: [summary])
        let replay = try #require(await journal.replayContent())
        #expect(replay.days.map(\.name) == ["Day 1"])
        #expect(replay.points.count == 2)
        #expect(replay.totalDistance < 200)
    }
}
