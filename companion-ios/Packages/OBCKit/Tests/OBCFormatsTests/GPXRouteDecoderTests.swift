import XCTest
import OBCDomain
@testable import OBCFormats

/// The GPX decoder — geometry, elevation, name/creator, and the waypoint
/// distance-along projection W1 renders. Ends on the real bundled sample
/// (a downsampled Komoot export) as the end-to-end pin.
final class GPXRouteDecoderTests: XCTestCase {
    private let decoder = GPXRouteDecoder()

    /// Three track points marching north 0.005° (~556 m) apart, plus two
    /// out-of-order waypoints the decoder must sort into ride order.
    private let sample = """
        <?xml version="1.0" encoding="UTF-8"?>
        <gpx version="1.1" creator="https://www.komoot.de" xmlns="http://www.topografix.com/GPX/1/1">
          <metadata><name>Test Tour</name></metadata>
          <wpt lat="47.0051" lon="11.0002"><name>Bakery stop</name><desc>Coffee</desc></wpt>
          <wpt lat="47.0001" lon="11.0000"><name>Trailhead</name></wpt>
          <trk><name>Track-level name</name><trkseg>
            <trkpt lat="47.000" lon="11.000"><ele>500.0</ele></trkpt>
            <trkpt lat="47.005" lon="11.000"><ele>521.5</ele></trkpt>
            <trkpt lat="47.010" lon="11.000"><ele>540.0</ele></trkpt>
          </trkseg></trk>
        </gpx>
        """

    func testDecodesTrackElevationNameAndCreator() throws {
        let route = try decoder.decode(Data(sample.utf8))

        XCTAssertEqual(route.name, "Test Tour", "metadata name wins over the track's")
        XCTAssertEqual(route.creator, "https://www.komoot.de")
        XCTAssertEqual(route.points.count, 3)
        XCTAssertEqual(route.points[1].coordinate.latitude, 47.005)
        XCTAssertEqual(route.points[1].elevationMeters, 521.5)
    }

    func testWaypointsAreProjectedAndOrderedAlongTheTrack() throws {
        let route = try decoder.decode(Data(sample.utf8))

        XCTAssertEqual(route.waypoints.map(\.name), ["Trailhead", "Bakery stop"],
                       "file order was reversed — ride order must win")
        XCTAssertEqual(route.waypoints.map(\.index), [0, 1])
        XCTAssertEqual(route.waypoints[0].distanceAlongMeters, 0)
        // The bakery sits nearest the middle track point, ~556 m along.
        XCTAssertEqual(route.waypoints[1].distanceAlongMeters, 556, accuracy: 10)
        XCTAssertEqual(route.waypoints[1].note, "Coffee")
        XCTAssertNil(route.waypoints[0].note, "no <desc> → no note")
    }

    func testRoutepointFallbackWhenThereIsNoTrack() throws {
        let rteOnly = """
            <gpx version="1.1" creator="test" xmlns="http://www.topografix.com/GPX/1/1">
              <rte><name>Old-style route</name>
                <rtept lat="47.0" lon="11.0"><ele>500</ele></rtept>
                <rtept lat="47.1" lon="11.0"><ele>600</ele></rtept>
              </rte>
            </gpx>
            """
        let route = try decoder.decode(Data(rteOnly.utf8))
        XCTAssertEqual(route.points.count, 2)
        XCTAssertEqual(route.name, "Old-style route")
    }

    func testMalformedXMLThrows() {
        XCTAssertThrowsError(try decoder.decode(Data("<gpx><trk>".utf8))) { error in
            guard case FormatError.malformed = error else {
                return XCTFail("expected .malformed, got \(error)")
            }
        }
    }

    func testNoGeometryThrows() {
        let empty = "<gpx version=\"1.1\" xmlns=\"http://www.topografix.com/GPX/1/1\"><metadata><name>x</name></metadata></gpx>"
        XCTAssertThrowsError(try decoder.decode(Data(empty.utf8))) { error in
            guard case FormatError.malformed = error else {
                return XCTFail("expected .malformed, got \(error)")
            }
        }
    }

    // MARK: Coordinate / elevation validation (#304)

    /// A non-finite coordinate (`lat="inf"`) parses as a `Double` but must be
    /// rejected as a clean `.malformed`, never built into a poisoning `NaN`
    /// coordinate.
    func testNonFiniteCoordinateRejectsTheFile() {
        let bad = """
            <gpx version="1.1" xmlns="http://www.topografix.com/GPX/1/1">
              <trk><trkseg>
                <trkpt lat="inf" lon="0.0"><ele>500</ele></trkpt>
              </trkseg></trk>
            </gpx>
            """
        assertMalformed(bad)
    }

    /// An out-of-range coordinate (`lat="999"`) is finite but not a valid
    /// latitude — also a clean reject.
    func testOutOfRangeCoordinateRejectsTheFile() {
        let bad = """
            <gpx version="1.1" xmlns="http://www.topografix.com/GPX/1/1">
              <trk><trkseg>
                <trkpt lat="999" lon="0.0"></trkpt>
                <trkpt lat="47.01" lon="11.0"></trkpt>
              </trkseg></trk>
            </gpx>
            """
        assertMalformed(bad)
    }

    /// The original crash path: a bad track coordinate poisons the cumulative
    /// distance → a `NaN` waypoint `along` → `sorted` traps. Rejecting the
    /// coordinate at the edge throws cleanly instead — no crash.
    func testBadCoordinateWithWaypointThrowsInsteadOfCrashing() {
        let bad = """
            <gpx version="1.1" xmlns="http://www.topografix.com/GPX/1/1">
              <wpt lat="47.005" lon="11.0002"><name>Bakery</name></wpt>
              <trk><trkseg>
                <trkpt lat="47.000" lon="11.000"></trkpt>
                <trkpt lat="nan" lon="11.000"></trkpt>
                <trkpt lat="47.010" lon="11.000"></trkpt>
              </trkseg></trk>
            </gpx>
            """
        assertMalformed(bad)
    }

    /// A non-finite `<ele>` is dropped to `nil` (no elevation) — the route still
    /// imports, it just carries no altitude for that point.
    func testNonFiniteElevationBecomesNil() throws {
        let mixed = """
            <gpx version="1.1" xmlns="http://www.topografix.com/GPX/1/1">
              <trk><trkseg>
                <trkpt lat="47.000" lon="11.000"><ele>inf</ele></trkpt>
                <trkpt lat="47.010" lon="11.000"><ele>540.0</ele></trkpt>
              </trkseg></trk>
            </gpx>
            """
        let route = try decoder.decode(Data(mixed.utf8))
        XCTAssertEqual(route.points.count, 2)
        XCTAssertNil(route.points[0].elevationMeters, "a non-finite <ele> is dropped, not stored")
        XCTAssertEqual(route.points[1].elevationMeters, 540.0)
    }

    private func assertMalformed(_ xml: String, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertThrowsError(try decoder.decode(Data(xml.utf8)), file: file, line: line) { error in
            guard case FormatError.malformed = error else {
                return XCTFail("expected .malformed, got \(error)", file: file, line: line)
            }
        }
    }

    /// End-to-end on the real bundled sample (`OBCMock/Fixtures/sample-import.gpx`,
    /// the `-OBCImportSample` file) — read via the repo path so this pins the
    /// exact bytes the E1 XCUITest imports.
    func testDecodesTheBundledKomootSample() throws {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()   // → Tests/OBCFormatsTests
            .deletingLastPathComponent()   // → Tests
            .deletingLastPathComponent()   // → OBCKit
            .appendingPathComponent("Sources/OBCMock/Fixtures/sample-import.gpx")
        let route = try decoder.decode(try Data(contentsOf: url))

        XCTAssertEqual(route.name, "Schwarzwald Tour · Tag 2")
        XCTAssertEqual(route.creator, "https://www.komoot.de")
        XCTAssertEqual(route.points.count, 150)
        XCTAssertEqual(route.waypoints.count, 5)
        XCTAssertTrue(route.points.allSatisfy { $0.elevationMeters != nil })
        // Ride order: monotonically increasing distance-along.
        let alongs = route.waypoints.map(\.distanceAlongMeters)
        XCTAssertEqual(alongs, alongs.sorted())
        XCTAssertEqual(route.waypoints.first?.name, "Steiler Abschnitt auf dem Murgtalradweg")
    }
}
