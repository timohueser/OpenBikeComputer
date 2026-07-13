import XCTest
import OBCDomain
@testable import OBCMock

/// The bundled JSON fixture sets load, map to domain types, and match the design's
/// sample data (so screenshots line up).
final class FixtureTests: XCTestCase {
    func testDefaultFixturesMatchDesignSampleData() {
        let set = FixtureSet.load("default")
        XCTAssertEqual(set.deviceInfo.name, "Trailhead")
        XCTAssertEqual(set.battery, 82)

        let kettle = set.routes.first { $0.summary.name == "Kettle Moraine Loop" }
        let route = try? XCTUnwrap(kettle)
        XCTAssertEqual(route?.summary.distanceMeters, 62_400)
        XCTAssertEqual(route?.summary.elevationGainMeters, 840)
        XCTAssertEqual(route?.summary.estimatedDuration, 12_000)   // 3h 20m
        XCTAssertEqual(route?.summary.source, .gpx)
        XCTAssertEqual(route?.waypoints.count, 4)
        XCTAssertEqual(route?.waypoints.first?.name, "Ottawa Lake trailhead")
        XCTAssertEqual(route?.waypoints.last?.distanceAlongMeters, 62_400)
    }

    func testRouteSummariesCarryANormalizedTrackPreview() {
        let set = FixtureSet.load("default")
        let preview = set.routes.first?.summary.trackPreview
        let points = preview?.points ?? []
        XCTAssertGreaterThan(points.count, 1)
        // Normalized into the unit square.
        for point in points {
            XCTAssertGreaterThanOrEqual(point.x, 0)
            XCTAssertLessThanOrEqual(point.x, 1)
            XCTAssertGreaterThanOrEqual(point.y, 0)
            XCTAssertLessThanOrEqual(point.y, 1)
        }
    }

    func testRidesDecodeWithDatesAndSpeeds() {
        let set = FixtureSet.load("default")
        let ride = set.rides.first { $0.summary.name == "Kettle Moraine Loop" }
        XCTAssertEqual(ride?.summary.distanceMeters, 58_200)
        XCTAssertEqual(ride?.summary.climbMeters, 812)
        XCTAssertNotNil(ride?.summary.date)
        XCTAssertGreaterThan(ride?.downloadByteCount ?? 0, 0)
    }

    func testEmptyFixturesAreEmptyButKeepTheDevice() {
        let set = FixtureSet.load("empty")
        XCTAssertTrue(set.routes.isEmpty)
        XCTAssertTrue(set.rides.isEmpty)
        XCTAssertEqual(set.deviceInfo.name, "Trailhead")
    }

    func testLargeFixturesHaveAManyRouteLibrary() {
        let set = FixtureSet.load("large")
        XCTAssertGreaterThanOrEqual(set.routes.count, 20)
    }

    func testUnknownFixtureNameFallsBackToBuiltIn() {
        let set = FixtureSet.load("does-not-exist")
        XCTAssertEqual(set.deviceInfo.name, "OBC (mock)")
        XCTAssertTrue(set.routes.isEmpty)
    }

    func testRouteBlobSynthesizesADeclaredSizePayload() {
        let set = FixtureSet.load("default")
        let entry = set.routes.first { $0.summary.name == "Kettle Moraine Loop" }
        let blob = entry?.blob()
        XCTAssertEqual(blob?.payload.count, 2_300_000)   // payloadBytes in the fixture
        XCTAssertEqual(blob?.waypoints.count, 4)
    }

    func testPayloadIsDeterministic() {
        XCTAssertEqual(MockPayload.make(count: 256), MockPayload.make(count: 256))
        XCTAssertEqual(MockPayload.make(count: 0).count, 0)
    }
}
