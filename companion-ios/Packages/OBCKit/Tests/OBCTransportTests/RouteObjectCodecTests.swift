import XCTest
import OBCDomain
import OBCFormats
@testable import OBCTransport

/// The route encoder: its OBCR v2 reader pinned against the shared
/// firmware-produced fixtures (`protocol-vectors/route-*.obcr`, decoded by the
/// production `obc-route` reader on the other side), plus encode→decode round-trips
/// proving geometry, exact stats, and waypoints survive an upload.
final class RouteObjectCodecTests: XCTestCase {
    /// `protocol-vectors/` at the repo root, resolved from this file's location.
    private static let vectorsDir = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // OBCTransportTests
        .deletingLastPathComponent()  // Tests
        .deletingLastPathComponent()  // OBCKit
        .deletingLastPathComponent()  // Packages
        .deletingLastPathComponent()  // companion-ios
        .deletingLastPathComponent()  // repo root
        .appendingPathComponent("protocol-vectors")

    private func fixture(_ name: String) throws -> Data {
        let url = Self.vectorsDir.appendingPathComponent(name)
        guard let data = FileManager.default.contents(atPath: url.path) else {
            XCTFail("fixture \(name) missing at \(url.path)")
            throw DeviceError.readFailed
        }
        return data
    }

    // MARK: Reader pinned against the firmware fixtures

    func testDecodesTheSharedWaypointsFixture() throws {
        let decoded = try RouteObjectCodec.decode(try fixture("route-waypoints.obcr"))

        // Header stats (manifest.json).
        XCTAssertEqual(decoded.version, 2)
        XCTAssertEqual(decoded.name, "Vector Loop")
        XCTAssertEqual(decoded.storedPointCount, 9)
        XCTAssertEqual(decoded.totalDistanceMeters, 2207)
        XCTAssertEqual(decoded.totalAscentMeters, 76)
        XCTAssertEqual(decoded.points.count, 9, "9 stored points, one chunk, seams counted once")

        // The camera-start point and the first vertex agree.
        XCTAssertEqual(decoded.start.latitude, 48.0, accuracy: 1e-5)
        XCTAssertEqual(decoded.points[0].coordinate.latitude, decoded.start.latitude, accuracy: 1e-9)

        // Waypoints (manifest.json): Brunnen at the start with an elevation, the
        // pass summit mid-route without one — both ride back as placed points.
        XCTAssertEqual(decoded.waypoints.count, 2)
        XCTAssertEqual(decoded.waypoints[0].name, "Brunnen")
        XCTAssertEqual(decoded.waypoints[0].distanceAlongMeters, 0)
        XCTAssertEqual(decoded.waypoints[0].coordinate.longitude, 7.8201, accuracy: 1e-5)
        XCTAssertEqual(decoded.waypoints[0].coordinate.latitude, 48.0001, accuracy: 1e-5)
        XCTAssertEqual(decoded.waypoints[1].name, "Pass Summit")
        XCTAssertEqual(decoded.waypoints[1].distanceAlongMeters, 1700)
    }

    func testPlainFixtureRidesIdenticallyMinusWaypoints() throws {
        let waypoints = try RouteObjectCodec.decode(try fixture("route-waypoints.obcr"))
        let plain = try RouteObjectCodec.decode(try fixture("route-plain.obcr"))

        XCTAssertTrue(plain.waypoints.isEmpty)
        // "must ride identically" — same name, stats, and geometry.
        XCTAssertEqual(plain.name, waypoints.name)
        XCTAssertEqual(plain.totalDistanceMeters, waypoints.totalDistanceMeters)
        XCTAssertEqual(plain.totalAscentMeters, waypoints.totalAscentMeters)
        XCTAssertEqual(plain.points, waypoints.points)
    }

    // MARK: Encode → decode round-trips

    func testRoundTripPreservesGeometryStatsAndWaypoints() throws {
        // A climb-then-descent so ascent and descent are both non-zero, with a
        // zig-zagging longitude so every vertex is a real turn the decimator keeps.
        let elevations: [Double] = [500, 512, 524, 540, 560, 548, 536, 520, 505]
        let points = elevations.enumerated().map { i, ele in
            RoutePoint(
                coordinate: Coordinate(latitude: 47.0 + 0.002 * Double(i), longitude: 11.0 + 0.001 * Double(i % 2)),
                elevationMeters: ele
            )
        }
        let route = ImportedRoute(
            name: "Round Trip Ridge", points: points,
            waypoints: [
                Waypoint(index: 0, name: "Trailhead", distanceAlongMeters: 0, coordinate: points[0].coordinate),
                Waypoint(index: 1, name: "Summit", distanceAlongMeters: 900, coordinate: points[4].coordinate),
            ]
        )

        let bytes = RouteObjectCodec.encode(route, name: route.name!)
        let decoded = try RouteObjectCodec.decode(bytes)

        XCTAssertEqual(decoded.version, 2)
        XCTAssertEqual(decoded.name, "Round Trip Ridge")

        // Exact stats mirror RouteStats (the detail-screen display) at whole-meter resolution.
        let stats = RouteStats.compute(from: points)
        XCTAssertEqual(Double(decoded.totalDistanceMeters), stats.distanceMeters, accuracy: 1)
        XCTAssertEqual(Double(decoded.totalAscentMeters), stats.elevationGainMeters, accuracy: 1)
        XCTAssertEqual(Double(decoded.totalDescentMeters), stats.elevationLossMeters, accuracy: 1)

        // Endpoints survive to microdegree precision; nothing is dropped or added
        // for this short, well-separated track.
        XCTAssertEqual(decoded.points.count, points.count)
        XCTAssertEqual(try XCTUnwrap(decoded.points.first).coordinate.latitude, 47.0, accuracy: 1e-6)
        XCTAssertEqual(
            try XCTUnwrap(decoded.points.last).coordinate.latitude,
            points.last!.coordinate.latitude, accuracy: 1e-6
        )

        // Waypoints round-trip name, distance-along, and coordinate.
        XCTAssertEqual(decoded.waypoints.map(\.name), ["Trailhead", "Summit"])
        XCTAssertEqual(decoded.waypoints[1].distanceAlongMeters, 900)
        XCTAssertEqual(decoded.waypoints[1].coordinate.longitude, points[4].coordinate.longitude, accuracy: 1e-6)
    }

    func testDecimationDropsCollinearInteriorPoints() throws {
        // A dead-straight, densely-sampled line: the decimator keeps only the
        // endpoints (everything else is within the epsilon of the chord).
        let points = (0...200).map { i in
            RoutePoint(coordinate: Coordinate(latitude: 47.0 + 0.0001 * Double(i), longitude: 11.0), elevationMeters: 300)
        }
        let decoded = try RouteObjectCodec.decode(RouteObjectCodec.encode(points: points, waypoints: [], name: "Straight"))
        XCTAssertLessThan(decoded.points.count, points.count, "collinear interior points are decimated away")
        XCTAssertGreaterThanOrEqual(decoded.points.count, 2)
        XCTAssertEqual(try XCTUnwrap(decoded.points.first).coordinate.latitude, 47.0, accuracy: 1e-6)
        XCTAssertEqual(
            try XCTUnwrap(decoded.points.last).coordinate.latitude,
            points.last!.coordinate.latitude, accuracy: 1e-6
        )
    }

    // MARK: A real GPX export → a compact OBCR (the point of B12)

    func testRealGPXExportEncodesToACompactRoute() throws {
        // The bundled Komoot export (a real GPX), decoded through the production
        // decoder — the exact path the app's import edge runs.
        let gpxURL = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // OBCTransportTests
            .deletingLastPathComponent()  // Tests
            .deletingLastPathComponent()  // OBCKit
            .appendingPathComponent("Sources/OBCMock/Fixtures/sample-import.gpx")
        let gpxData = try XCTUnwrap(FileManager.default.contents(atPath: gpxURL.path))
        let route = try GPXRouteDecoder().decode(gpxData)

        let obcr = RouteObjectCodec.encode(route, name: route.name ?? "Route")

        // The old placeholder was `distanceMeters × 37` bytes of zeros — the real
        // encoder is a small fraction of it, and a few kB in absolute terms (a route
        // is a handful of bytes per stored vertex, not per metre).
        let placeholder = Int(RouteStats.compute(from: route.points).distanceMeters * 37)
        XCTAssertLessThan(obcr.count, placeholder / 10, "OBCR is far smaller than the old zero-filled placeholder")
        XCTAssertLessThan(obcr.count, 50_000, "a real export encodes to tens of kB, not MB")

        // …and it round-trips back to the same route.
        let decoded = try RouteObjectCodec.decode(obcr)
        XCTAssertEqual(decoded.name, route.name)
        XCTAssertEqual(decoded.waypoints.count, route.waypoints.count)
        XCTAssertGreaterThan(decoded.totalDistanceMeters, 0)
        XCTAssertGreaterThanOrEqual(decoded.points.count, 2)
    }

    func testEmptyGeometryEncodesToEmptyData() {
        XCTAssertTrue(RouteObjectCodec.encode(points: [], waypoints: [], name: "Nothing").isEmpty)
    }

    func testRejectsNonOBCRBytes() {
        XCTAssertThrowsError(try RouteObjectCodec.decode(Data([0, 1, 2, 3, 4, 5])))
        XCTAssertThrowsError(try RouteObjectCodec.decode(Data("OBCR".utf8)))  // magic but truncated
    }
}
