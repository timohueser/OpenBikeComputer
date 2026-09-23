import Foundation
import OBCDomain
import Testing

/// Photo placement on a ride due north: one point every 10 s and 10 m, so a time and a
/// distance read the same number.
struct RidePhotoPlacementTests {
    private static let start = Date(timeIntervalSince1970: 1_790_000_000)
    private static let metersPerDegree = 111_320.0

    /// `ys` are metres north of the start and `xs` metres east, one point per 10 s. The point at
    /// `gap` resumes the recording 10 minutes later.
    private func points(_ ys: [Double], xs: [Double]? = nil, gapAt gap: Int? = nil, step: Double = 10) -> [RidePoint] {
        ys.enumerated().map { index, y in
            let pause = gap.map { index >= $0 ? 600.0 : 0 } ?? 0
            return RidePoint(
                timestamp: time(Double(index) * step + pause), coordinate: place(north: y, east: xs?[index] ?? 0),
                segmentStart: index == gap
            )
        }
    }

    private func straight(step: Double = 10) -> [RidePoint] {
        points((0...100).map { Double($0) * 10 }, step: step)
    }

    private func time(_ seconds: Double) -> Date { Self.start.addingTimeInterval(seconds) }

    private func place(north: Double, east: Double = 0) -> Coordinate {
        Coordinate(
            latitude: 46.5 + north / Self.metersPerDegree,
            longitude: 8.4 + east / (Self.metersPerDegree * cos(46.5 * .pi / 180))
        )
    }

    private func place(_ candidates: [PhotoCandidate], on points: [RidePoint]) -> [RidePhotoPlacement.Placed] {
        RidePhotoPlacement.place(candidates, on: points, line: MeasuredLine(ridePoints: points))
    }

    private func placeOne(_ candidate: PhotoCandidate, on points: [RidePoint]) -> RidePhotoPlacement.Placed? {
        place([candidate], on: points).first
    }

    @Test func aPhotoIsPlacedByTimeBetweenPoints() throws {
        let placed = try #require(placeOne(PhotoCandidate(assetID: "a", takenAt: time(505)), on: straight()))

        #expect(abs(placed.distanceMeters - 505) < 1)
        #expect(placed.coordinate.distance(to: place(north: 505)) < 1)
        #expect(!placed.locationOffTrack)
    }

    /// Out 500 m on the east lane and back on the west lane, 6 m apart. The geotag sits on the out
    /// lane, but the time is on the way back.
    @Test func anOutAndBackKeepsTheLegOfThePhotoTime() throws {
        let ride = points((0...100).map { Double(50 - abs(50 - $0)) * 10 }, xs: (0...100).map { $0 <= 50 ? 3 : -3 })
        let candidate = PhotoCandidate(assetID: "a", takenAt: time(790), location: place(north: 210, east: 3))

        let placed = try #require(placeOne(candidate, on: ride))

        #expect(placed.distanceMeters == MeasuredLine(ridePoints: ride).vertices[79].distance)
        #expect(!placed.locationOffTrack)
    }

    @Test func aFarGeotagIsMarkedAndDoesNotMoveThePhoto() throws {
        let candidate = PhotoCandidate(assetID: "a", takenAt: time(300), location: place(north: 300, east: 5_000))

        let placed = try #require(placeOne(candidate, on: straight()))

        #expect(abs(placed.distanceMeters - 300) < 1)
        #expect(placed.locationOffTrack)
    }

    /// Ten minutes either side of the ride belong to it, at its start or its end.
    @Test func theMarginSitsAtTheStartAndTheEnd() {
        let ride = straight()
        let placed = place(
            [PhotoCandidate(assetID: "tooEarly", takenAt: time(-601)), PhotoCandidate(assetID: "early", takenAt: time(-600)),
             PhotoCandidate(assetID: "late", takenAt: time(1_600)), PhotoCandidate(assetID: "tooLate", takenAt: time(1_601))],
            on: ride
        )

        #expect(placed.map(\.photo.assetID) == ["early", "late"])
        #expect(placed.map(\.distanceMeters) == [0, MeasuredLine(ridePoints: ride).length])
        #expect(placed.map(\.coordinate) == [ride[0].coordinate, ride[100].coordinate])
    }

    @Test func aPhotoInARecordingPauseSitsWhereTheRiderStopped() throws {
        let ride = points((0...100).map { Double($0) * 10 }, gapAt: 50)

        let placed = try #require(placeOne(PhotoCandidate(assetID: "a", takenAt: time(800)), on: ride))

        #expect(abs(placed.distanceMeters - 490) < 1)
        #expect(placed.coordinate == ride[49].coordinate)
    }

    /// A trim keeps the photos in the kept part at their places and drops the rest. One point a
    /// minute, so the trimmed parts are longer than the margin.
    @Test func aTrimMovesNoPhotoAndDropsTheTrimmedOnes() {
        let photos = [600.0, 3_000, 5_400].map { RidePhoto(assetID: "\(Int($0))", takenAt: time($0)) }
        let ride = straight(step: 60)
        let trimmed = Array(ride[30...70])

        let before = RidePhotoPlacement.place(photos, on: ride, line: MeasuredLine(ridePoints: ride))
        let after = RidePhotoPlacement.place(photos, on: trimmed, line: MeasuredLine(ridePoints: trimmed))

        #expect(after.map(\.id) == ["3000"])
        #expect(after.first?.coordinate == before[1].coordinate)
        #expect(abs((after.first?.distanceMeters ?? 0) - 200) < 1)
    }

    /// A split part keeps the photos of its own time.
    @Test func eachSplitPartGetsItsPhotos() {
        let photos = [600.0, 3_000, 5_400].map { RidePhoto(assetID: "\(Int($0))", takenAt: time($0)) }
        let ride = straight(step: 60)
        let first = Array(ride[...20]), second = Array(ride[21...])

        let placedFirst = RidePhotoPlacement.place(photos, on: first, line: MeasuredLine(ridePoints: first))
        let placedSecond = RidePhotoPlacement.place(photos, on: second, line: MeasuredLine(ridePoints: second))

        #expect(placedFirst.map(\.id) == ["600"])
        #expect(placedSecond.map(\.id) == ["3000", "5400"])
        #expect(abs((placedSecond.first?.distanceMeters ?? 0) - 290) < 1)
    }

    @Test func photosComeBackInTimeOrder() {
        let candidates = [900.0, 100, 500].map { PhotoCandidate(assetID: "\(Int($0))", takenAt: time($0)) }

        #expect(place(candidates, on: straight()).map(\.photo.assetID) == ["100", "500", "900"])
    }
}
