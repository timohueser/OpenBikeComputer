import XCTest
import OBCDomain
@testable import OBCFormats

/// The TCX decoder — course geometry, altitude, name/author, and the course-point
/// waypoints (incl. the PointType name fallback). Ends on the bundled sample
/// (the `-OBCImportSample tcx` file) as the end-to-end pin.
final class TCXRouteDecoderTests: XCTestCase {
    private let decoder = TCXRouteDecoder()

    /// Three trackpoints marching north 0.005° (~556 m) apart, plus two
    /// out-of-order course points the decoder must sort into ride order —
    /// one named, one falling back to its PointType.
    private let sample = """
        <?xml version="1.0" encoding="UTF-8"?>
        <TrainingCenterDatabase xmlns="http://www.garmin.com/xmlschemas/TrainingCenterDatabase/v2">
          <Courses>
            <Course>
              <Name>Test Course</Name>
              <Track>
                <Trackpoint>
                  <Time>2026-05-30T08:30:00Z</Time>
                  <Position><LatitudeDegrees>47.000</LatitudeDegrees><LongitudeDegrees>11.000</LongitudeDegrees></Position>
                  <AltitudeMeters>500.0</AltitudeMeters>
                  <DistanceMeters>0.0</DistanceMeters>
                </Trackpoint>
                <Trackpoint>
                  <Position><LatitudeDegrees>47.005</LatitudeDegrees><LongitudeDegrees>11.000</LongitudeDegrees></Position>
                  <AltitudeMeters>521.5</AltitudeMeters>
                </Trackpoint>
                <Trackpoint>
                  <Position><LatitudeDegrees>47.010</LatitudeDegrees><LongitudeDegrees>11.000</LongitudeDegrees></Position>
                  <AltitudeMeters>540.0</AltitudeMeters>
                </Trackpoint>
              </Track>
              <CoursePoint>
                <Name>Bakery stop</Name>
                <Position><LatitudeDegrees>47.0051</LatitudeDegrees><LongitudeDegrees>11.0002</LongitudeDegrees></Position>
                <PointType>Food</PointType>
                <Notes>Coffee</Notes>
              </CoursePoint>
              <CoursePoint>
                <Name></Name>
                <Position><LatitudeDegrees>47.0001</LatitudeDegrees><LongitudeDegrees>11.0000</LongitudeDegrees></Position>
                <PointType>Left</PointType>
              </CoursePoint>
            </Course>
          </Courses>
          <Author xsi:type="Application_t" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
            <Name>Garmin Connect</Name>
          </Author>
        </TrainingCenterDatabase>
        """

    func testDecodesTrackAltitudeNameAndAuthor() throws {
        let route = try decoder.decode(Data(sample.utf8))

        XCTAssertEqual(route.name, "Test Course")
        XCTAssertEqual(route.creator, "Garmin Connect")
        XCTAssertEqual(route.points.count, 3)
        XCTAssertEqual(route.points[1].coordinate.latitude, 47.005)
        XCTAssertEqual(route.points[1].elevationMeters, 521.5)
    }

    func testCoursePointsAreProjectedAndOrderedAlongTheTrack() throws {
        let route = try decoder.decode(Data(sample.utf8))

        XCTAssertEqual(route.waypoints.map(\.name), ["Left", "Bakery stop"],
                       "file order was reversed — ride order must win; empty Name falls back to PointType")
        XCTAssertEqual(route.waypoints.map(\.index), [0, 1])
        XCTAssertEqual(route.waypoints[0].distanceAlongMeters, 0)
        // The bakery sits nearest the middle track point, ~556 m along.
        XCTAssertEqual(route.waypoints[1].distanceAlongMeters, 556, accuracy: 10)
        XCTAssertEqual(route.waypoints[1].note, "Coffee")
        XCTAssertNil(route.waypoints[0].note, "no <Notes> → no note")
    }

    /// A shared *workout* TCX (Activities, no Courses) still imports — its
    /// trackpoints become the route; there's just no course name or waypoints.
    func testActivityTrackFallbackWhenThereIsNoCourse() throws {
        let activityOnly = """
            <TrainingCenterDatabase xmlns="http://www.garmin.com/xmlschemas/TrainingCenterDatabase/v2">
              <Activities><Activity Sport="Biking"><Id>2026-05-30T08:30:00Z</Id><Lap><Track>
                <Trackpoint><Position><LatitudeDegrees>47.0</LatitudeDegrees><LongitudeDegrees>11.0</LongitudeDegrees></Position><AltitudeMeters>500</AltitudeMeters></Trackpoint>
                <Trackpoint><Position><LatitudeDegrees>47.1</LatitudeDegrees><LongitudeDegrees>11.0</LongitudeDegrees></Position><AltitudeMeters>600</AltitudeMeters></Trackpoint>
              </Track></Lap></Activity></Activities>
            </TrainingCenterDatabase>
            """
        let route = try decoder.decode(Data(activityOnly.utf8))
        XCTAssertEqual(route.points.count, 2)
        XCTAssertNil(route.name)
        XCTAssertTrue(route.waypoints.isEmpty)
    }

    /// A trackpoint without a position (paused GPS — TCX allows it) is skipped,
    /// not decoded as (0, 0).
    func testPositionlessTrackpointsAreSkipped() throws {
        let gappy = """
            <TrainingCenterDatabase xmlns="http://www.garmin.com/xmlschemas/TrainingCenterDatabase/v2">
              <Courses><Course><Name>Gappy</Name><Track>
                <Trackpoint><Position><LatitudeDegrees>47.0</LatitudeDegrees><LongitudeDegrees>11.0</LongitudeDegrees></Position></Trackpoint>
                <Trackpoint><Time>2026-05-30T08:31:00Z</Time><AltitudeMeters>510</AltitudeMeters></Trackpoint>
                <Trackpoint><Position><LatitudeDegrees>47.1</LatitudeDegrees><LongitudeDegrees>11.0</LongitudeDegrees></Position></Trackpoint>
              </Track></Course></Courses>
            </TrainingCenterDatabase>
            """
        let route = try decoder.decode(Data(gappy.utf8))
        XCTAssertEqual(route.points.count, 2)
    }

    func testMalformedXMLThrows() {
        XCTAssertThrowsError(try decoder.decode(Data("<TrainingCenterDatabase><Courses>".utf8))) { error in
            guard case FormatError.malformed = error else {
                return XCTFail("expected .malformed, got \(error)")
            }
        }
    }

    func testNoGeometryThrows() {
        let empty = """
            <TrainingCenterDatabase xmlns="http://www.garmin.com/xmlschemas/TrainingCenterDatabase/v2">
              <Courses><Course><Name>Empty</Name></Course></Courses>
            </TrainingCenterDatabase>
            """
        XCTAssertThrowsError(try decoder.decode(Data(empty.utf8))) { error in
            guard case FormatError.malformed = error else {
                return XCTFail("expected .malformed, got \(error)")
            }
        }
    }

    /// End-to-end on the real bundled sample (`OBCMock/Fixtures/sample-import.tcx`,
    /// the `-OBCImportSample tcx` file) — read via the repo path so this pins the
    /// exact bytes the E1 XCUITest imports.
    func testDecodesTheBundledCourseSample() throws {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()   // → Tests/OBCFormatsTests
            .deletingLastPathComponent()   // → Tests
            .deletingLastPathComponent()   // → OBCKit
            .appendingPathComponent("Sources/OBCMock/Fixtures/sample-import.tcx")
        let route = try decoder.decode(try Data(contentsOf: url))

        XCTAssertEqual(route.name, "Alpe d'Huez Climb")
        XCTAssertEqual(route.creator, "Garmin Connect")
        XCTAssertEqual(route.points.count, 60)
        XCTAssertEqual(route.waypoints.count, 3)
        XCTAssertTrue(route.points.allSatisfy { $0.elevationMeters != nil })
        // Ride order: the file lists Summit first; the decoder must re-sort.
        XCTAssertEqual(route.waypoints.map(\.name), ["Turn 21", "Water", "Summit"])
        let alongs = route.waypoints.map(\.distanceAlongMeters)
        XCTAssertEqual(alongs, alongs.sorted())
    }
}
