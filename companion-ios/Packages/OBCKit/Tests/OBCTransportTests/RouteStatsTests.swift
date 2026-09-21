import XCTest
import OBCDomain

/// `RouteStats`: the derived geometry behind the stat strip and the summary an import saves.
/// Synthetic tracks with known answers.
final class RouteStatsTests: XCTestCase {
    /// 0.01° of latitude ≈ 1112 m at any longitude.
    private func track(_ elevations: [Double?]) -> [RoutePoint] {
        elevations.enumerated().map { index, ele in
            RoutePoint(
                coordinate: Coordinate(latitude: 47.0 + 0.01 * Double(index), longitude: 11.0),
                elevationMeters: ele
            )
        }
    }

    func testDistanceIsCumulativeStoredRouteMetric() {
        let stats = RouteStats.compute(from: track([nil, nil, nil]))
        XCTAssertEqual(stats.distanceMeters, 2 * 1112, accuracy: 5)
    }

    func testClimbAndDescentUseHysteresisAgainstJitter() {
        // The shared 3 m band ignores the 103 m dip: 100 → 105 → 110.
        let stats = RouteStats.compute(from: track([100, 101, 100.5, 105, 103, 110]))
        XCTAssertEqual(stats.elevationGainMeters, 10, accuracy: 0.001)
        XCTAssertEqual(stats.elevationLossMeters, 0, accuracy: 0.001)
    }

    func testMaxGradeOverSustainedWindow() {
        // Steady 100 m of climb over about 4448 m is ~2.2%; one steep leg of 60 m over a single
        // ~1112 m segment is ~5.4% and must win.
        let stats = RouteStats.compute(from: track([0, 25, 50, 110, 135]))
        XCTAssertEqual(stats.maxGradePercent ?? 0, 60.0 / 1112 * 100, accuracy: 0.5)
    }

    func testNoElevationMeansNoClimbNoGradeNoProfile() {
        let stats = RouteStats.compute(from: track([nil, nil, nil]))
        XCTAssertEqual(stats.elevationGainMeters, 0)
        XCTAssertNil(stats.maxGradePercent)
        XCTAssertTrue(stats.elevationProfile.isEmpty)
    }

    func testProfileDownsamplesKeepingEndpoints() {
        let samples = (0..<200).map(Double.init)
        let profile = RouteStats.downsample(samples, to: 64)
        XCTAssertEqual(profile.count, 64)
        XCTAssertEqual(profile.first, 0)
        XCTAssertEqual(profile.last, 199)
    }

    func testEstimateIsTouringPace() {
        // A flat track: distance at 16 km/h, with no climb penalty.
        let points = (0...29).map {  // 29 × ~1112 m ≈ 32.2 km
            RoutePoint(coordinate: Coordinate(latitude: 47.0 + 0.01 * Double($0), longitude: 11.0))
        }
        let stats = RouteStats.compute(from: points)
        XCTAssertEqual(stats.estimatedDuration, stats.distanceMeters / 1000 / 16 * 3600, accuracy: 1)
    }
}
