import XCTest
import OBCDomain
import OBCFormats
@testable import OBCTransport

/// The route encoder (B12, #286): its OBCR v3 reader pinned against the shared
/// firmware-produced fixtures (`specs/vectors/route-*.obcr`, decoded by the
/// production `obc-route` reader on the other side), plus encode→decode round-trips
/// proving geometry, exact stats, and waypoints survive an upload.
final class RouteObjectCodecTests: XCTestCase {
    /// `specs/vectors/`, resolved from this file's location.
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

        // Header stats (manifest.json).
        XCTAssertEqual(decoded.version, 3)
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

        // …and the v3 fields the firmware converter wrote (manifest.json): the
        // fountain's `<sym>Drinking Water</sym>` mapped to Water and it sits 13 m
        // **left** of travel; the summit's `<type>Viewpoint</type>` is unmapped
        // (generic) and it sits on a track vertex, so on-route.
        XCTAssertEqual(decoded.waypoints[0].category, .water)
        XCTAssertEqual(decoded.waypoints[0].lateralOffsetMeters, -13)
        XCTAssertNil(decoded.waypoints[1].category)
        XCTAssertEqual(decoded.waypoints[1].lateralOffsetMeters, 0)
    }

    /// The v3 bump is breaking on both sides: the same bytes labelled v1 or v2 are
    /// refused rather than read with the old 40-byte record.
    func testRejectsPreV3Routes() throws {
        var bytes = try fixture("route-waypoints.obcr")
        for old: UInt8 in [1, 2] {
            bytes[bytes.startIndex + 4] = old
            XCTAssertThrowsError(try RouteObjectCodec.decode(bytes), "v\(old) must not decode")
        }
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
                Waypoint(
                    index: 1, name: "Summit", distanceAlongMeters: 900, coordinate: points[4].coordinate,
                    category: .water, lateralOffsetMeters: -120.4
                ),
            ]
        )

        let bytes = RouteObjectCodec.encode(route, name: route.name!)
        let decoded = try RouteObjectCodec.decode(bytes)

        XCTAssertEqual(decoded.version, 3)
        XCTAssertEqual(decoded.name, "Round Trip Ridge")

        // Exact stats mirror RouteStats (the E1 display) at whole-meter resolution.
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

        // Waypoints round-trip name, distance-along, coordinate, category and the
        // signed offset (stored as whole metres, so −120.4 comes back as −120).
        XCTAssertEqual(decoded.waypoints.map(\.name), ["Trailhead", "Summit"])
        XCTAssertEqual(decoded.waypoints[1].distanceAlongMeters, 900)
        XCTAssertEqual(decoded.waypoints[1].coordinate.longitude, points[4].coordinate.longitude, accuracy: 1e-6)
        XCTAssertNil(decoded.waypoints[0].category, "an uncategorized waypoint stays generic")
        XCTAssertEqual(decoded.waypoints[0].lateralOffsetMeters, 0)
        XCTAssertEqual(decoded.waypoints[1].category, .water)
        XCTAssertEqual(decoded.waypoints[1].lateralOffsetMeters, -120)
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

    // MARK: Encoder-determinism pin (V6, #770)

    /// Adopt-by-content (#770) re-links an unlinked device copy by comparing a
    /// **fresh** OBCR re-encode's CRC-32 against the catalog, so byte-determinism
    /// is load-bearing: were the encoder to emit even one different byte for an
    /// unchanged route, adoption would silently degrade to re-upload — safe, but
    /// it "looks like the badges went dark." This pins it: two encodes must be
    /// byte-identical, their CRC must match the golden below, and `payloadCRC`
    /// (the exact function adoption calls) must agree. An encoder change then
    /// **fails here loudly** and must re-pin `goldenCRC` consciously.
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

        let first = RouteObjectCodec.encode(route, name: route.name!)
        let second = RouteObjectCodec.encode(route, name: route.name!)
        XCTAssertEqual(first, second, "the OBCR encode must be byte-identical run to run")

        // The stored fixture CRC — an encoder change must re-pin this on purpose,
        // not dim adoption silently. Re-pinned for OBCR v3 (#947: version byte 3,
        // the 44-byte waypoint record, and this waypoint's category + signed
        // offset now inside it).
        let goldenCRC: UInt32 = 0x6B3F_72E3
        XCTAssertEqual(
            CRC32.checksum(first), goldenCRC,
            "OBCR encoding changed; adoption's re-encode CRC moved — re-pin goldenCRC consciously")

        // `payloadCRC(for:)` is the one CRC adoption compares against the catalog
        // — it must equal the raw encode's CRC (same geometry, waypoints, name).
        let record = PlannedRouteRecord(
            summary: RouteSummary(
                id: RouteID("pin"), name: "Determinism Pin",
                distanceMeters: 0, elevationGainMeters: 0),
            route: route, sourceFileName: "pin.gpx", sourceFileData: Data())
        XCTAssertEqual(RouteObjectCodec.payloadCRC(for: record), goldenCRC)
    }
}
