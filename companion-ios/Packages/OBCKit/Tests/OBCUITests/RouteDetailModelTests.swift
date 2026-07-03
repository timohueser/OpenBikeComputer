import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The detail model's three dressings against `MockTransport` — the
/// library-first planned render (waypoints + profile from the record, no
/// device round-trip), the tracked profile fill, the per-dressing stat
/// strips, rename, and the save summary.
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

    // MARK: Planned

    func testPlannedRendersFromItsLibraryRecordWithNoDeviceRoundTrip() async {
        let control = makeControl()
        let entry = control.fixtures.routes[0]  // Kettle Moraine Loop
        // What RootView threads in: the saved record's own detail — planned
        // is library-first, so the device is never asked for it.
        let model = RouteDetailModel(
            transport: MockTransport(control: control),
            dressing: .planned(entry.summary),
            preloadedDetail: entry.detail()
        )

        XCTAssertEqual(model.name, "Kettle Moraine Loop")
        XCTAssertEqual(model.tag.text, "Planned")
        XCTAssertFalse(model.tag.isAccent)
        XCTAssertTrue(model.isRenamable)
        XCTAssertNil(model.importedFromLine)
        // Everything renders before (and without) any transport round-trip.
        XCTAssertEqual(model.waypoints.count, 4)
        XCTAssertEqual(model.waypoints.first?.name, "Ottawa Lake trailhead")
        XCTAssertEqual(model.elevationProfile.count, 10)
        XCTAssertEqual(model.maxGradePercent, 9)

        model.start()
        try? await Task.sleep(for: .milliseconds(50))
        XCTAssertEqual(model.waypoints.count, 4, "start() must not clobber the record's detail")
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

    /// Upload is link-bound: `canUpload` follows the live connection stream —
    /// the button dims when the device isn't actually there.
    func testCanUploadFollowsTheLiveConnection() async {
        let control = makeControl()
        let route = control.fixtures.routes[0].summary
        let model = RouteDetailModel(transport: MockTransport(control: control), dressing: .planned(route))

        model.start()
        await waitFor("connected replay") { model.canUpload }

        control.connection = .outOfRange
        await waitFor("link-down gate") { !model.canUpload }
        control.connection = .connected
        await waitFor("link-up gate") { model.canUpload }
    }

    /// The moment an upload commits, the model pins the assigned id — a
    /// second Upload on the same screen targets it (the device replaces the
    /// object) instead of sending "new" again, and the button reads up to
    /// date until the content moves (a rename out-dates it).
    func testUploadCommitPinsTheTargetAndStateFollowsContent() async {
        let control = makeControl()
        let entry = control.fixtures.routes[2]  // Blue Mounds — not on the device
        let model = RouteDetailModel(
            transport: MockTransport(control: control),
            dressing: .planned(entry.summary),
            preloadedDetail: entry.detail(),
            plannedGeometry: ImportedRoute(
                name: entry.summary.name, points: entry.points, waypoints: entry.waypoints
            )
        )
        XCTAssertEqual(model.deviceCopyState, .notOnDevice)
        XCTAssertNil(model.makeUploadBlob().targetObjectID, "a fresh route uploads as new")

        let committed = model.makeUploadBlob()
        model.recordUploaded(objectID: 42, crc32: CRC32.checksum(committed.payload))
        XCTAssertEqual(model.deviceCopyState, .upToDate)
        XCTAssertEqual(model.makeUploadBlob().targetObjectID, 42, "a re-upload replaces, never duplicates")

        XCTAssertTrue(model.rename(to: "Blue Mounds (shortcut)"))
        XCTAssertEqual(model.deviceCopyState, .outdated, "a rename out-dates the device copy")
        XCTAssertEqual(model.makeUploadBlob().targetObjectID, 42, "…and the update still targets the same object")
    }

    func testTrackedDetailReadFailureDegradesQuietly() async {
        let control = makeControl()
        let ride = control.fixtures.rides[0].summary
        control.failNextOp(.readFailed)
        let model = RouteDetailModel(transport: MockTransport(control: control), dressing: .tracked(ride))

        model.start()
        try? await Task.sleep(for: .milliseconds(100))
        XCTAssertTrue(model.elevationProfile.isEmpty, "no profile card on a failed read")
        XCTAssertEqual(model.name, ride.name, "summary content stays up")
    }

    // MARK: Tracked

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

    /// A threaded `rideGeometry` feeds the interactive map at full
    /// resolution; without it, the map falls back to the (downsampled)
    /// preview's coordinates rather than showing nothing.
    func testTrackedMapCoordinatesUseTheThreadedGeometryOrFallBackToThePreview() {
        let control = makeControl()
        let ride = control.fixtures.rides[0].summary
        let fullTrack = (0..<500).map { Coordinate(latitude: 47.0 + 0.0001 * Double($0), longitude: 11.0) }

        let withGeometry = RouteDetailModel(
            transport: MockTransport(control: control), dressing: .tracked(ride), rideGeometry: fullTrack
        )
        XCTAssertEqual(withGeometry.mapCoordinates, fullTrack, "full resolution, not the ride card's preview cap")

        let withoutGeometry = RouteDetailModel(
            transport: MockTransport(control: control), dressing: .tracked(ride)
        )
        XCTAssertEqual(
            withoutGeometry.mapCoordinates, ride.trackPreview?.coordinates ?? [],
            "no threaded geometry → the preview's coordinates, not an empty map"
        )
    }

    // MARK: Imported

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

    /// The imported dressing's interactive map draws the full parsed
    /// geometry, never the `preview`'s 256-point downsample — the whole point
    /// of threading `mapCoordinates` separately.
    func testImportedMapCoordinatesAreFullResolutionNotThePreviewCap() {
        let points = (0..<1_000).map {
            RoutePoint(coordinate: Coordinate(latitude: 47.0 + 0.0001 * Double($0), longitude: 11.0))
        }
        let route = ImportedRoute(name: "Long Tour", points: points)
        let model = RouteDetailModel(
            transport: MockTransport(control: makeControl()),
            dressing: .imported(route, fileName: "long.gpx")
        )

        XCTAssertEqual(model.mapCoordinates.count, 1_000, "full resolution for the interactive map")
        XCTAssertLessThan(
            model.preview?.points.count ?? 0, 1_000,
            "the compact preview stays downsampled — that cap is intentional for the thumbnail"
        )
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

    // MARK: Upload blob

    func testUploadBlobCarriesRenameWaypointsAndRealOBCR() async throws {
        let control = makeControl()
        let entry = control.fixtures.routes[0]  // Kettle Moraine Loop, 62.4 km
        // A planned route re-uploads the library's parsed geometry (threaded in).
        let model = RouteDetailModel(
            transport: MockTransport(control: control),
            dressing: .planned(entry.summary),
            preloadedDetail: entry.detail(),
            plannedGeometry: importedRoute
        )
        XCTAssertTrue(model.rename(to: "Kettle Gravel Day"))

        let blob = model.makeUploadBlob()
        XCTAssertEqual(blob.summary.id, entry.summary.id)
        XCTAssertEqual(blob.summary.name, "Kettle Gravel Day", "a rename must ride along")
        XCTAssertEqual(blob.waypoints.count, 4)

        // The payload is a real OBCR file — decodes back with the rename +
        // waypoints, a few kB.
        let decoded = try RouteObjectCodec.decode(blob.payload)
        XCTAssertEqual(decoded.name, "Kettle Gravel Day")
        XCTAssertEqual(decoded.waypoints.count, 4)
        XCTAssertLessThan(blob.payload.count, 10_000)
    }

    func testPlannedReuploadTargetsTheDeviceObjectID() {
        // A planned route already on the device re-uploads with its object id so
        // the device replaces it in place instead of duplicating.
        let control = makeControl()
        let model = RouteDetailModel(
            transport: MockTransport(control: control),
            dressing: .planned(control.fixtures.routes[0].summary),
            plannedGeometry: importedRoute,
            deviceObjectID: 7
        )
        XCTAssertEqual(model.makeUploadBlob().targetObjectID, 7)
    }

    func testPlannedUploadWithoutGeometrySendsNothing() {
        // A device-listed route the phone never imported has no app-side geometry.
        let route = RouteSummary(id: RouteID("42"), name: "On Device", distanceMeters: 40_000, elevationGainMeters: 300)
        let model = RouteDetailModel(transport: MockTransport(control: makeControl()), dressing: .planned(route))
        XCTAssertTrue(model.makeUploadBlob().payload.isEmpty)
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

    // MARK: Rename

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
