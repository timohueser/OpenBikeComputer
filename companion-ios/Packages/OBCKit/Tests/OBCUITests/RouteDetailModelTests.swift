import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// B4 acceptance, host-side: the detail model's three dressings against
/// `MockTransport` — summary-first render, the async detail fill (waypoints +
/// elevation), the per-dressing stat strips, rename, and the E1 save summary.
@MainActor
final class RouteDetailModelTests: XCTestCase {
    private func makeControl() -> MockControl {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        return control
    }

    private func waitFor(
        _ what: String,
        timeout: Duration = .seconds(5),
        _ condition: () -> Bool
    ) async {
        let deadline = ContinuousClock.now.advanced(by: timeout)
        while !condition() {
            if ContinuousClock.now > deadline {
                XCTFail("timed out waiting for \(what)")
                return
            }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    // MARK: E2 · planned

    func testPlannedRendersSummaryThenFillsDetail() async {
        let control = makeControl()
        let route = control.fixtures.routes[0].summary  // Kettle Moraine Loop
        let model = RouteDetailModel(transport: MockTransport(control: control), dressing: .planned(route))

        // Summary facts render before any transport round-trip.
        XCTAssertEqual(model.name, "Kettle Moraine Loop")
        XCTAssertEqual(model.tag.text, "Planned")
        XCTAssertFalse(model.tag.isAccent)
        XCTAssertTrue(model.waypoints.isEmpty)
        XCTAssertTrue(model.isRenamable)
        XCTAssertNil(model.importedFromLine)

        model.start()
        await waitFor("detail fill") { !model.waypoints.isEmpty }
        XCTAssertEqual(model.waypoints.count, 4)
        XCTAssertEqual(model.waypoints.first?.name, "Ottawa Lake trailhead")
        XCTAssertEqual(model.elevationProfile.count, 10)
        XCTAssertEqual(model.maxGradePercent, 9)
    }

    func testPlannedStatStripMatchesTheDesignColumns() {
        let control = makeControl()
        let route = control.fixtures.routes[0].summary
        let model = RouteDetailModel(transport: MockTransport(control: control), dressing: .planned(route))

        XCTAssertEqual(model.stats.map(\.key), ["Distance", "Climb", "Est. time", "Max"])
        // Numbers stay locale-aware (a German phone reads "62,4") — pin the
        // wiring against the formatter, not an en-US literal.
        XCTAssertEqual(model.stats[0].value, OBCFormat.distanceValue(meters: 62_400))
        XCTAssertEqual(model.stats[0].unit, "km")
        XCTAssertEqual(model.stats[2].value, "3:20")
        // MAX shows an em dash until the detail read lands the grade.
        XCTAssertEqual(model.stats[3].value, "—")
    }

    func testDetailReadFailureDegradesQuietly() async {
        let control = makeControl()
        let route = control.fixtures.routes[0].summary
        control.failNextOp(.readFailed)
        let model = RouteDetailModel(transport: MockTransport(control: control), dressing: .planned(route))

        model.start()
        try? await Task.sleep(for: .milliseconds(100))
        XCTAssertTrue(model.waypoints.isEmpty, "no waypoints row on a failed read")
        XCTAssertEqual(model.name, "Kettle Moraine Loop", "summary content stays up")
    }

    // MARK: E3 · tracked

    func testTrackedDressingShowsRideStatsAndFillsProfile() async {
        let control = makeControl()
        let ride = control.fixtures.rides[0].summary  // Kettle Moraine Loop (ride)
        let model = RouteDetailModel(transport: MockTransport(control: control), dressing: .tracked(ride))

        XCTAssertEqual(model.stats.map(\.key), ["Distance", "Moving", "Avg", "Climb"])
        XCTAssertTrue(model.tag.text.hasPrefix("Tracked · "))
        XCTAssertTrue(model.tag.isAccent)
        XCTAssertNotNil(model.subtitle)
        XCTAssertTrue(model.isRenamable)

        model.start()
        await waitFor("ride profile") { !model.elevationProfile.isEmpty }
        XCTAssertEqual(model.elevationProfile.count, 9)
    }

    // MARK: E1 · imported

    private var importedRoute: ImportedRoute {
        // ~1112 m per step; rises 5 steps then falls 4 — both climb and
        // descent are non-zero for the stat strip.
        let elevations: [Double] = [500, 510, 520, 530, 540, 550, 540, 530, 520, 510]
        let points = elevations.enumerated().map { index, ele in
            RoutePoint(
                coordinate: Coordinate(latitude: 47.0 + 0.01 * Double(index), longitude: 11.0),
                elevationMeters: ele
            )
        }
        return ImportedRoute(
            name: "Schwarzwald Tour · Tag 2",
            creator: "https://www.komoot.de",
            points: points,
            waypoints: [
                Waypoint(index: 0, name: "Start", distanceAlongMeters: 0, coordinate: points[0].coordinate),
                Waypoint(index: 1, name: "Pass", distanceAlongMeters: 5_000, coordinate: points[5].coordinate),
            ]
        )
    }

    func testImportedComputesEverythingUpFront() {
        let model = RouteDetailModel(
            transport: MockTransport(control: makeControl()),
            dressing: .imported(importedRoute, fileName: "schwarzwald.gpx")
        )

        XCTAssertEqual(model.name, "Schwarzwald Tour · Tag 2")
        XCTAssertEqual(model.subtitle, "schwarzwald.gpx")
        XCTAssertEqual(model.tag.text, "New · unsaved")
        XCTAssertEqual(model.importedFromLine, "Imported from Komoot")
        XCTAssertFalse(model.isRenamable)
        XCTAssertEqual(model.waypoints.count, 2)
        XCTAssertEqual(model.elevationProfile.count, 10)
        XCTAssertEqual(model.stats.map(\.key), ["Distance", "Climb", "Descent", "Est. time"])
        XCTAssertEqual(model.stats[1].value, OBCFormat.climbValue(meters: 50))
        XCTAssertEqual(model.stats[2].value, OBCFormat.climbValue(meters: 40))
        XCTAssertEqual(model.distanceMeters, 9 * 1112.0, accuracy: 20)
    }

    func testImportedFromLineFallsBackToTheFileType() {
        var route = importedRoute
        route.creator = "RideWithGPS"
        let model = RouteDetailModel(
            transport: MockTransport(control: makeControl()),
            dressing: .imported(route, fileName: "tour.gpx")
        )
        XCTAssertEqual(model.importedFromLine, "Imported from GPX file")
    }

    func testImportedFromLineRecognizesGarmin() {
        var route = importedRoute
        route.creator = "Garmin Connect"
        let model = RouteDetailModel(
            transport: MockTransport(control: makeControl()),
            dressing: .imported(route, fileName: "course.tcx")
        )
        XCTAssertEqual(model.importedFromLine, "Imported from Garmin")
    }

    func testMakeSummaryCarriesTheParsedStats() {
        let model = RouteDetailModel(
            transport: MockTransport(control: makeControl()),
            dressing: .imported(importedRoute, fileName: "schwarzwald.gpx")
        )
        let summary = model.makeSummary()

        XCTAssertEqual(summary.name, "Schwarzwald Tour · Tag 2")
        XCTAssertEqual(summary.source, .gpx)
        XCTAssertEqual(summary.pointCount, 10)
        XCTAssertEqual(summary.distanceMeters, model.distanceMeters)
        XCTAssertNotNil(summary.trackPreview)
        XCTAssertTrue(summary.id.rawValue.hasPrefix("imported-"))
    }

    func testMakeDetailKeepsWaypointsAndProfileForTheSave() {
        let model = RouteDetailModel(
            transport: MockTransport(control: makeControl()),
            dressing: .imported(importedRoute, fileName: "schwarzwald.gpx")
        )
        let detail = model.makeDetail()

        XCTAssertEqual(detail.waypoints.count, 2)
        XCTAssertEqual(detail.elevationProfile.count, 10)
        XCTAssertEqual(detail.summary.name, "Schwarzwald Tour · Tag 2")
        XCTAssertNotNil(detail.maxGradePercent)
    }

    // MARK: Upload blob (B5)

    func testUploadBlobCarriesRenameWaypointsAndAPlausiblePayload() async {
        let control = makeControl()
        let route = control.fixtures.routes[0].summary  // Kettle Moraine Loop, 62.4 km
        let model = RouteDetailModel(transport: MockTransport(control: control), dressing: .planned(route))
        model.start()
        await waitFor("detail fill") { !model.waypoints.isEmpty }
        XCTAssertTrue(model.rename(to: "Kettle Gravel Day"))

        let blob = model.makeUploadBlob()
        XCTAssertEqual(blob.summary.id, route.id)
        XCTAssertEqual(blob.summary.name, "Kettle Gravel Day", "a rename must ride along")
        XCTAssertEqual(blob.waypoints.count, 4)
        // Placeholder payload at the design scale: 62.4 km reads as "2.3 MB".
        XCTAssertEqual(
            OBCFormat.megabytesValue(blob.payload.count, locale: Locale(identifier: "en_US")), "2.3"
        )
    }

    func testUploadBlobAndSaveDetailShareTheImportedID() {
        let model = RouteDetailModel(
            transport: MockTransport(control: makeControl()),
            dressing: .imported(importedRoute, fileName: "schwarzwald.gpx")
        )
        // "Uploading saves it too": what went to the device and what lands in
        // the library must be the same route.
        XCTAssertEqual(model.makeUploadBlob().summary.id, model.makeDetail().summary.id)
    }

    func testPreloadedDetailSkipsTheTransportFetch() async {
        let control = makeControl()
        // A phone-only id: the mock would throw for it — preload must cover.
        let summary = RouteSummary(
            id: RouteID("imported-abc"), name: "Saved Import",
            distanceMeters: 10_000, elevationGainMeters: 50
        )
        let detail = RouteDetail(
            summary: summary,
            waypoints: [Waypoint(index: 0, name: "Start", distanceAlongMeters: 0,
                                 coordinate: Coordinate(latitude: 47, longitude: 11))],
            elevationProfile: [500, 550, 520],
            maxGradePercent: 6
        )
        let model = RouteDetailModel(
            transport: MockTransport(control: control),
            dressing: .planned(summary),
            preloadedDetail: detail
        )

        XCTAssertEqual(model.waypoints.count, 1, "preloaded waypoints must render immediately")
        XCTAssertEqual(model.elevationProfile, [500, 550, 520])
        XCTAssertEqual(model.maxGradePercent, 6)

        model.start()
        try? await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(model.waypoints.count, 1, "start() must not clobber the preload")
    }

    // MARK: H12 · rename

    func testRenameTrimsAndRejectsEmpty() {
        let control = makeControl()
        let model = RouteDetailModel(
            transport: MockTransport(control: control),
            dressing: .planned(control.fixtures.routes[0].summary)
        )

        XCTAssertTrue(model.rename(to: "  Kettle Gravel Day  "))
        XCTAssertEqual(model.name, "Kettle Gravel Day")
        XCTAssertFalse(model.rename(to: "   "))
        XCTAssertEqual(model.name, "Kettle Gravel Day")
    }
}
