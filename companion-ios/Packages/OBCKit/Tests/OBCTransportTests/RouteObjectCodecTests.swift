import XCTest
import OBCDomain
import OBCFormats
@testable import OBCTransport

/// The OBCR v5 route encoder and reader: the reader is pinned against the shared
/// firmware-produced fixtures in `specs/vectors`, and encode-decode round-trips prove geometry,
/// exact stats and waypoints survive an upload.
final class RouteObjectCodecTests: XCTestCase {
    private static let vectorsDir = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // OBCTransportTests
        .deletingLastPathComponent()  // Tests
        .deletingLastPathComponent()  // OBCKit
        .deletingLastPathComponent()  // Packages
        .deletingLastPathComponent()  // companion-ios
        .deletingLastPathComponent()  // repo root
        .appendingPathComponent("specs/vectors")

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

        XCTAssertEqual(decoded.version, 5)
        XCTAssertEqual(decoded.name, "Vector Loop")
        XCTAssertEqual(decoded.storedPointCount, 9)
        XCTAssertEqual(decoded.totalDistanceMeters, 2207)
        XCTAssertEqual(decoded.totalAscentMeters, 76)
        XCTAssertEqual(decoded.points.count, 9, "9 stored points, one chunk, seams counted once")

        XCTAssertEqual(decoded.start.latitude, 48.0, accuracy: 1e-5)
        XCTAssertEqual(decoded.points[0].coordinate.latitude, decoded.start.latitude, accuracy: 1e-9)

        XCTAssertEqual(decoded.waypoints.count, 2)
        XCTAssertEqual(decoded.waypoints[0].name, "Brunnen")
        XCTAssertEqual(decoded.waypoints[0].distanceAlongMeters, 0)
        XCTAssertEqual(decoded.waypoints[0].coordinate.longitude, 7.8201, accuracy: 1e-5)
        XCTAssertEqual(decoded.waypoints[0].coordinate.latitude, 48.0001, accuracy: 1e-5)
        XCTAssertEqual(decoded.waypoints[1].name, "Pass Summit")
        XCTAssertEqual(decoded.waypoints[1].distanceAlongMeters, 1700)

        // The fixture's fountain has `<sym>Drinking Water</sym>` and sits 13 m left of travel.
        // The summit's `<type>Viewpoint</type>` is unmapped and sits on a vertex, so on-route.
        XCTAssertEqual(decoded.waypoints[0].category, .water)
        XCTAssertEqual(decoded.waypoints[0].lateralOffsetMeters, -13)
        XCTAssertNil(decoded.waypoints[1].category)
        XCTAssertEqual(decoded.waypoints[1].lateralOffsetMeters, 0)
    }

    func testRejectsOlderVersions() throws {
        var bytes = try fixture("route-waypoints.obcr")
        for old: UInt8 in [1, 2, 3, 4] {
            bytes[bytes.startIndex + 4] = old
            XCTAssertThrowsError(try RouteObjectCodec.decode(bytes), "v\(old) must not decode")
        }
    }

    func testPlainFixtureRidesIdenticallyMinusWaypoints() throws {
        let waypoints = try RouteObjectCodec.decode(try fixture("route-waypoints.obcr"))
        let plain = try RouteObjectCodec.decode(try fixture("route-plain.obcr"))

        XCTAssertTrue(plain.waypoints.isEmpty)
        XCTAssertEqual(plain.name, waypoints.name)
        XCTAssertEqual(plain.totalDistanceMeters, waypoints.totalDistanceMeters)
        XCTAssertEqual(plain.totalAscentMeters, waypoints.totalAscentMeters)
        XCTAssertEqual(plain.points, waypoints.points)
        var candidate = try fixture("route-plain.obcr")
        candidate[5] |= 8
        XCTAssertEqual(try RouteObjectCodec.decode(candidate).points, plain.points)
        candidate[5] |= 16
        XCTAssertThrowsError(try RouteObjectCodec.decode(candidate))
    }

    // MARK: Encode → decode round-trips

    func testRoundTripPreservesGeometryStatsAndWaypoints() throws {
        // A climb then a descent, so ascent and descent are both non-zero, with a zig-zag
        // longitude so the decimator keeps every vertex.
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
                Waypoint(
                    index: 1, name: "Summit", distanceAlongMeters: 900, coordinate: points[4].coordinate,
                    category: .water, lateralOffsetMeters: -120.4
                ),
            ]
        )

        let bytes = RouteObjectCodec.encode(route, name: route.name!, bikeType: .road)
        let decoded = try RouteObjectCodec.decode(bytes)

        XCTAssertEqual(decoded.version, 5)
        XCTAssertEqual(decoded.name, "Round Trip Ridge")

        // The stats mirror RouteStats at whole-metre resolution.
        let stats = RouteStats.compute(from: points)
        XCTAssertEqual(Double(decoded.totalDistanceMeters), stats.distanceMeters, accuracy: 1)
        XCTAssertEqual(Double(decoded.totalAscentMeters), stats.elevationGainMeters, accuracy: 1)
        XCTAssertEqual(Double(decoded.totalDescentMeters), stats.elevationLossMeters, accuracy: 1)

        // Nothing is dropped or added for this short, well-separated track.
        XCTAssertEqual(decoded.points.count, points.count)
        XCTAssertEqual(try XCTUnwrap(decoded.points.first).coordinate.latitude, 47.0, accuracy: 1e-6)
        XCTAssertEqual(
            try XCTUnwrap(decoded.points.last).coordinate.latitude,
            points.last!.coordinate.latitude, accuracy: 1e-6
        )

        // The lateral offset is stored as whole metres, so -120.4 comes back as -120.
        XCTAssertEqual(decoded.waypoints.map(\.name), ["Trailhead", "Summit"])
        XCTAssertEqual(decoded.waypoints[1].distanceAlongMeters, 900)
        XCTAssertEqual(decoded.waypoints[1].coordinate.longitude, points[4].coordinate.longitude, accuracy: 1e-6)
        XCTAssertNil(decoded.waypoints[0].category, "an uncategorized waypoint stays generic")
        XCTAssertEqual(decoded.waypoints[0].lateralOffsetMeters, 0)
        XCTAssertEqual(decoded.waypoints[1].category, .water)
        XCTAssertEqual(decoded.waypoints[1].lateralOffsetMeters, -120)
    }

    func testDecimationDropsCollinearInteriorPoints() throws {
        // A dead-straight, densely sampled line: every interior point is within the chord epsilon.
        let points = (0...200).map { i in
            RoutePoint(coordinate: Coordinate(latitude: 47.0 + 0.0001 * Double(i), longitude: 11.0), elevationMeters: 300)
        }
        let decoded = try RouteObjectCodec.decode(RouteObjectCodec.encode(points: points, waypoints: [], name: "Straight", bikeType: .road))
        XCTAssertLessThan(decoded.points.count, points.count, "collinear interior points are decimated away")
        XCTAssertGreaterThanOrEqual(decoded.points.count, 2)
        XCTAssertEqual(try XCTUnwrap(decoded.points.first).coordinate.latitude, 47.0, accuracy: 1e-6)
        XCTAssertEqual(
            try XCTUnwrap(decoded.points.last).coordinate.latitude,
            points.last!.coordinate.latitude, accuracy: 1e-6
        )
    }

    // MARK: A real GPX export to a compact OBCR

    func testRealGPXExportEncodesToACompactRoute() throws {
        // A real Komoot export, decoded through the production decoder: the app's import path.
        let gpxURL = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // OBCTransportTests
            .deletingLastPathComponent()  // Tests
            .deletingLastPathComponent()  // OBCKit
            .appendingPathComponent("Sources/OBCMock/Fixtures/sample-import.gpx")
        let gpxData = try XCTUnwrap(FileManager.default.contents(atPath: gpxURL.path))
        let route = try GPXRouteDecoder().decode(gpxData)

        let obcr = RouteObjectCodec.encode(route, name: route.name ?? "Route", bikeType: .road)

        // The placeholder is a zero-filled bytes-per-metre estimate; OBCR costs bytes per vertex.
        let placeholder = Int(RouteStats.compute(from: route.points).distanceMeters * 37)
        XCTAssertLessThan(obcr.count, placeholder / 10, "OBCR is far smaller than the old zero-filled placeholder")
        XCTAssertLessThan(obcr.count, 50_000, "a real export encodes to tens of kB, not MB")

        let decoded = try RouteObjectCodec.decode(obcr)
        XCTAssertEqual(decoded.name, route.name)
        XCTAssertEqual(decoded.waypoints.count, route.waypoints.count)
        XCTAssertGreaterThan(decoded.totalDistanceMeters, 0)
        XCTAssertGreaterThanOrEqual(decoded.points.count, 2)
    }

    func testMissingCoverageAndProvenanceSurviveTheCodec() throws {
        let source = WaypointProvenance(store: Data(repeating: 1, count: 16), object: 2, revision: 3, ordinal: 4)
        let points = [
            RoutePoint(coordinate: Coordinate(latitude: 0, longitude: 0), elevationMeters: 0),
            RoutePoint(coordinate: Coordinate(latitude: 0, longitude: 0.0005), elevationMeters: nil, surface: 3),
            RoutePoint(coordinate: Coordinate(latitude: 0, longitude: 0.001), elevationMeters: 100, surface: 3, elevationIncomplete: true),
        ]
        let waypoint = Waypoint(index: 0, name: "Source", distanceAlongMeters: 0, coordinate: points[0].coordinate, provenance: source)
        let bytes = RouteObjectCodec.encode(points: points, waypoints: [waypoint], name: "Gap", bikeType: .road)
        let decoded = try RouteObjectCodec.decode(bytes)
        XCTAssertEqual(decoded.points[0].elevationMeters, 0)
        XCTAssertNil(decoded.points[1].elevationMeters)
        XCTAssertTrue(decoded.points[2].elevationIncomplete)
        XCTAssertEqual(decoded.points[2].surface, 3)
        XCTAssertEqual(decoded.totalAscentMeters, 0)
        XCTAssertEqual(decoded.waypoints[0].provenance, source)
        var invalid = bytes
        invalid[118] = 1
        XCTAssertThrowsError(try RouteObjectCodec.decode(invalid))
        invalid = bytes
        invalid[128] = 1
        XCTAssertThrowsError(try RouteObjectCodec.decode(invalid))
    }

    func testDensifyingIncompleteElevationKeepsOnlyTheMeasuredEndpoints() throws {
        let points = [
            RoutePoint(coordinate: Coordinate(latitude: 47, longitude: 8), elevationMeters: 100),
            RoutePoint(coordinate: Coordinate(latitude: 47, longitude: 8.1), elevationMeters: 200, surface: 1, elevationIncomplete: true),
        ]
        let decoded = try RouteObjectCodec.decode(RouteObjectCodec.encode(points: points, waypoints: [], name: "Gap", bikeType: .road))
        XCTAssertGreaterThan(decoded.points.count, 2)
        XCTAssertEqual(decoded.points.first?.elevationMeters, 100)
        XCTAssertEqual(decoded.points.last?.elevationMeters, 200)
        for point in decoded.points.dropFirst().dropLast() {
            XCTAssertNil(point.elevationMeters, "a synthetic point is not a measured endpoint")
        }
        XCTAssertTrue(decoded.points.dropFirst().allSatisfy(\.elevationIncomplete))
        XCTAssertEqual(decoded.totalAscentMeters, 0)
    }

    func testDescriptorEnvelopesMatchTheSharedOverlapContract() throws {
        let valid = try RouteObjectCodec.decode(try fixture("route-visit.obcr"))
        XCTAssertNotNil(valid.visitDescriptor)
        for name in ["route-visit-waypoint-overlap.obcr", "route-visit-index-overlap.obcr"] {
            XCTAssertThrowsError(try RouteObjectCodec.decode(try fixture(name)), name)
        }
    }

    func testEmptyGeometryEncodesToEmptyData() {
        XCTAssertTrue(RouteObjectCodec.encode(points: [], waypoints: [], name: "Nothing", bikeType: .road).isEmpty)
    }

    func testRejectsNonOBCRBytes() {
        XCTAssertThrowsError(try RouteObjectCodec.decode(Data([0, 1, 2, 3, 4, 5])))
        XCTAssertThrowsError(try RouteObjectCodec.decode(Data("OBCR".utf8)))  // magic but truncated
    }

    // MARK: Encoder-determinism pin

    /// Adoption re-links an unlinked device copy by comparing a fresh OBCR re-encode's CRC-32
    /// against the catalog, so byte-determinism is load-bearing: one different byte would silently
    /// degrade adoption to a re-upload. An encoder change must re-pin `goldenCRC` on purpose.
    func testEncoderIsByteDeterministicForAdoption() {
        let elevations: [Double] = [500, 512, 524, 540, 560, 548, 536, 520, 505]
        let points = elevations.enumerated().map { i, ele in
            RoutePoint(
                coordinate: Coordinate(
                    latitude: 47.0 + 0.002 * Double(i), longitude: 11.0 + 0.001 * Double(i % 2)),
                elevationMeters: ele)
        }
        let route = ImportedRoute(
            name: "Determinism Pin", points: points,
            waypoints: [Waypoint(index: 0, name: "Start", distanceAlongMeters: 0,
                                 coordinate: points[0].coordinate,
                                 category: .water, lateralOffsetMeters: -42)])

        let first = RouteObjectCodec.encode(route, name: route.name!, bikeType: .road)
        let second = RouteObjectCodec.encode(route, name: route.name!, bikeType: .road)
        XCTAssertEqual(first, second, "the OBCR encode must be byte-identical run to run")

        let goldenCRC: UInt32 = 0xAA1B381C
        XCTAssertEqual(
            CRC32.checksum(first), goldenCRC,
            "OBCR encoding changed; adoption's re-encode CRC moved — re-pin goldenCRC consciously")

        // `payloadCRC(for:)` is the CRC adoption compares, so it must equal the raw encode's CRC.
        let record = PlannedRouteRecord(
            summary: RouteSummary(
                id: RouteID("pin"), name: "Determinism Pin",
                distanceMeters: 0, elevationGainMeters: 0),
            route: route, sourceFileName: "pin.gpx", sourceFileData: Data())
        XCTAssertEqual(RouteObjectCodec.payloadCRC(for: record), goldenCRC)
    }
}
