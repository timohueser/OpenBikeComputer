import Foundation
import OBCDomain
import Testing

/// Photo placement on a ride due north: one point every 10 s and 10 m, so a time and a
/// distance read the same number.
struct RidePhotoPlacementTests {
    private static let start = Date(timeIntervalSince1970: 1_790_000_000)
    private static let metersPerDegree = 111_320.0

    /// `ys` are metres north of the start, one point per 10 s.
    private func points(_ ys: [Double], gapAt gap: Int? = nil) -> [RidePoint] {
        ys.enumerated().map { index, y in
            // A pause: the recording resumes 10 minutes later.
            let pause = gap.map { index >= $0 ? 600.0 : 0 } ?? 0
            return RidePoint(
                timestamp: time(Double(index) * 10 + pause), coordinate: place(north: y),
                segmentStart: index == gap
            )
        }
    }

    private func straight(_ count: Int = 101) -> [RidePoint] {
        points((0..<count).map { Double($0) * 10 })
    }

    private func time(_ seconds: Double) -> Date { Self.start.addingTimeInterval(seconds) }

    private func place(north: Double, east: Double = 0) -> Coordinate {
        Coordinate(
            latitude: 46.5 + north / Self.metersPerDegree,
            longitude: 8.4 + east / (Self.metersPerDegree * cos(46.5 * .pi / 180))
        )
    }

    private func placeOne(_ candidate: PhotoCandidate, on points: [RidePoint]) -> RidePhotoPlacement.Placed? {
        RidePhotoPlacement.place([candidate], on: points).first
    }

    @Test func aPhotoWithoutGeotagIsPlacedByTimeBetweenPoints() throws {
        let placed = try #require(placeOne(PhotoCandidate(assetID: "a", takenAt: time(505)), on: straight()))

        #expect(abs(placed.photo.distanceMeters - 505) < 1)
        #expect(!placed.locationOffTrack)
    }

    /// The camera clock says 100 s, the geotag says 800 m: a geotag near the track wins.
    @Test func aGeotagNearTheTrackPlacesThePhotoAtTheNearestTrackPoint() throws {
        let candidate = PhotoCandidate(assetID: "a", takenAt: time(100), location: place(north: 800, east: 250))

        let placed = try #require(placeOne(candidate, on: straight()))

        #expect(abs(placed.photo.distanceMeters - 800) < 1)
        #expect(!placed.locationOffTrack)
    }

    @Test func aFarGeotagDuringTheRideIsPlacedByTimeAndMarked() throws {
        let candidate = PhotoCandidate(assetID: "a", takenAt: time(300), location: place(north: 300, east: 5_000))

        let placed = try #require(placeOne(candidate, on: straight()))

        #expect(abs(placed.photo.distanceMeters - 300) < 1)
        #expect(placed.locationOffTrack)
    }

    /// Ten minutes either side of the ride belong to it, at its start or its end. There, a far
    /// geotag means the photo is from somewhere else.
    @Test func theMarginAroundTheRide() {
        let ride = straight()
        let before = time(-5 * 60), after = time(1_000 + 5 * 60)
        let candidates = [
            PhotoCandidate(assetID: "early", takenAt: before),
            PhotoCandidate(assetID: "late", takenAt: after),
            PhotoCandidate(assetID: "farEarly", takenAt: before, location: place(north: 0, east: 5_000)),
            PhotoCandidate(assetID: "tooEarly", takenAt: time(-11 * 60)),
            PhotoCandidate(assetID: "tooLate", takenAt: time(1_000 + 11 * 60)),
        ]

        let placed = RidePhotoPlacement.place(candidates, on: ride)

        #expect(placed.map(\.photo.assetID) == ["early", "late"])
        #expect(placed.first?.photo.distanceMeters == 0)
        #expect(placed.last?.photo.distanceMeters == MeasuredLine(ridePoints: ride).length)
        #expect(RidePhotoPlacement.window(for: ride) == time(-600)...time(1_600))
    }

    /// Out 500 m and back on the same road: the geotag fits both legs, and the time picks one.
    @Test func anOutAndBackKeepsTheLegThePhotoWasTakenOn() throws {
        let ride = points((0...100).map { Double(50 - abs(50 - $0)) * 10 })
        let candidate = PhotoCandidate(assetID: "a", takenAt: time(790), location: place(north: 205, east: 20))

        let placed = try #require(placeOne(candidate, on: ride))

        #expect(abs(placed.photo.distanceMeters - 795) < 1)
    }

    @Test func aPhotoInARecordingPauseSitsWhereTheRiderStopped() throws {
        let ride = points((0..<101).map { Double($0) * 10 }, gapAt: 50)

        let placed = try #require(placeOne(PhotoCandidate(assetID: "a", takenAt: time(800)), on: ride))

        #expect(abs(placed.photo.distanceMeters - 490) < 1)
    }

    @Test func candidatesComeBackInTimeOrder() {
        let candidates = [900.0, 100, 500].map { PhotoCandidate(assetID: "\(Int($0))", takenAt: time($0)) }

        #expect(RidePhotoPlacement.place(candidates, on: straight()).map(\.photo.assetID) == ["100", "500", "900"])
    }
}
