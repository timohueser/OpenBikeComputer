import XCTest
import OBCDomain

/// `TrackPreview.normalizing` — the projection shared by the mock fixtures and the
/// real decode that feeds the `GPSTrackPreview` (B11).
final class GeoTests: XCTestCase {
    func testEmptyTrackIsEmptyPreview() {
        let preview = TrackPreview.normalizing([])
        XCTAssertTrue(preview.points.isEmpty)
        XCTAssertEqual(preview.aspectRatio, 1)
    }

    func testSinglePointCentres() {
        let preview = TrackPreview.normalizing([Coordinate(latitude: 47, longitude: 8)])
        XCTAssertEqual(preview.points, [TrackPreview.Point(x: 0.5, y: 0.5)])
    }

    func testNorthwardLineMapsSouthToBottomNorthToTop() {
        // Constant longitude, increasing latitude (heading north).
        let preview = TrackPreview.normalizing([
            Coordinate(latitude: 0, longitude: 0),
            Coordinate(latitude: 1, longitude: 0),
        ])
        // y is flipped (y-down): the southern point sits at the bottom (y≈1), the
        // northern point at the top (y≈0).
        XCTAssertEqual(preview.points.first!.y, 1, accuracy: 1e-9)
        XCTAssertEqual(preview.points.last!.y, 0, accuracy: 1e-9)
    }

    func testAllPointsWithinUnitSquare() {
        let coords = (0..<200).map { i in
            Coordinate(latitude: 47 + Double(i) * 0.01, longitude: 8 + sin(Double(i)) * 0.02)
        }
        let preview = TrackPreview.normalizing(coords)
        for point in preview.points {
            XCTAssert((0...1).contains(point.x), "x out of range: \(point.x)")
            XCTAssert((0...1).contains(point.y), "y out of range: \(point.y)")
        }
        XCTAssertGreaterThan(preview.aspectRatio, 0)
    }

    func testDownsamplesToMaxPoints() {
        let coords = (0..<1000).map { Coordinate(latitude: Double($0) * 0.001, longitude: 0) }
        XCTAssertEqual(TrackPreview.normalizing(coords, maxPoints: 100).points.count, 100)
        // Fewer points than the cap are kept as-is.
        let few = (0..<10).map { Coordinate(latitude: Double($0), longitude: 0) }
        XCTAssertEqual(TrackPreview.normalizing(few, maxPoints: 100).points.count, 10)
    }

    func testDegenerateBoundingBoxCentres() {
        let coords = Array(repeating: Coordinate(latitude: 5, longitude: 5), count: 4)
        let preview = TrackPreview.normalizing(coords)
        for point in preview.points {
            XCTAssertEqual(point.x, 0.5, accuracy: 1e-9)
            XCTAssertEqual(point.y, 0.5, accuracy: 1e-9)
        }
    }

    // MARK: Coordinates carrier (#294 — the MapKit basemap path)

    func testRetainsSourceCoordinatesAlignedWithPoints() {
        let coords = [
            Coordinate(latitude: 42.0, longitude: -88.0),
            Coordinate(latitude: 42.1, longitude: -88.1),
            Coordinate(latitude: 42.2, longitude: -88.05),
        ]
        let preview = TrackPreview.normalizing(coords)
        // Same count as the unit-square points, and the actual source lat/lon.
        XCTAssertEqual(preview.coordinates.count, preview.points.count)
        XCTAssertEqual(preview.coordinates, coords)
    }

    func testDownsampleKeepsCoordinatesAlignedWithPoints() {
        let coords = (0..<1000).map { Coordinate(latitude: Double($0) * 0.001, longitude: 0) }
        let preview = TrackPreview.normalizing(coords, maxPoints: 100)
        XCTAssertEqual(preview.coordinates.count, 100)
        XCTAssertEqual(preview.coordinates.count, preview.points.count)
    }

    func testEmptyAndSinglePointCoordinates() {
        XCTAssertTrue(TrackPreview.normalizing([]).coordinates.isEmpty)
        let one = [Coordinate(latitude: 47, longitude: 8)]
        XCTAssertEqual(TrackPreview.normalizing(one).coordinates, one)
    }

    // MARK: normalizingShared — the trip card's multi-stage preview (TR6)

    func testNormalizingSharedPutsAllStagesInOneUnitSquare() {
        // Two disjoint tracks: A in the north-west, B in the south-east.
        let a = [Coordinate(latitude: 48.0, longitude: 8.0), Coordinate(latitude: 48.0, longitude: 8.4)]
        let b = [Coordinate(latitude: 47.0, longitude: 8.6), Coordinate(latitude: 47.0, longitude: 9.0)]
        let shared = TrackPreview.normalizingShared([a, b])

        XCTAssertEqual(shared.count, 2)
        // One shared bbox: every point is within the unit square, and across BOTH
        // stages the extremes touch 0 and 1 (north/top, south/bottom; west/east).
        let allPoints = shared.flatMap(\.points)
        let xs = allPoints.map(\.x)
        let ys = allPoints.map(\.y)
        XCTAssertEqual(xs.min() ?? .nan, 0, accuracy: 1e-9)
        XCTAssertEqual(xs.max() ?? .nan, 1, accuracy: 1e-9)
        XCTAssertEqual(ys.min() ?? .nan, 0, accuracy: 1e-9)  // northmost (stage A) at top
        XCTAssertEqual(ys.max() ?? .nan, 1, accuracy: 1e-9)  // southmost (stage B) at bottom
        // A single shared aspect ratio.
        XCTAssertEqual(shared[0].aspectRatio, shared[1].aspectRatio)
        // Coordinates ride through per stage for the basemap path.
        XCTAssertEqual(shared[0].coordinates, a)
        XCTAssertEqual(shared[1].coordinates, b)
    }

    func testNormalizingSharedMapsEmptyTracksToEmpty() {
        let one = [Coordinate(latitude: 47, longitude: 8), Coordinate(latitude: 47.1, longitude: 8.1)]
        let shared = TrackPreview.normalizingShared([[], one, []])
        XCTAssertTrue(shared[0].points.isEmpty)
        XCTAssertFalse(shared[1].points.isEmpty)
        XCTAssertTrue(shared[2].points.isEmpty)
        // All-empty input → all empty (nothing to draw).
        XCTAssertTrue(TrackPreview.normalizingShared([[], []]).allSatisfy { $0.points.isEmpty })
    }
}
