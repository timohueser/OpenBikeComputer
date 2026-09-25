import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The detail model's three dressings against `MockTransport`: the library-first planned render
/// (waypoints and profile from the record, with no device round-trip), the tracked profile,
/// the per-dressing stat strips, rename, and the save summary.
@MainActor
final class RouteDetailModelTests: XCTestCase {
    private func makeControl() -> MockControl {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        return control
    }

    // MARK: Planned

    func testPlannedRendersFromItsLibraryRecordWithNoDeviceRoundTrip() {
        let control = makeControl()
        let transport = MockTransport(control: control)
        let entry = control.fixtures.routes[0]  // Kettle Moraine Loop
        // What RootView threads in: the saved record's own detail. Planned is library-first.
        let model = RouteDetailModel(
            transport: transport,
            dressing: .planned(entry.summary),
            preloadedDetail: entry.detail()
        )

        XCTAssertEqual(model.name, "Kettle Moraine Loop")
        XCTAssertTrue(model.isRenamable)
        XCTAssertNil(model.subtitle, "no source file was threaded in")
        XCTAssertEqual(model.waypoints.count, 4)
        XCTAssertEqual(model.waypoints.first?.name, "Ottawa Lake trailhead")
        XCTAssertEqual(model.elevationProfile.count, 10)
        XCTAssertEqual(model.maxGradePercent, 9)

        model.start()
        XCTAssertEqual(model.waypoints.count, 4, "start() must not clobber the record's detail")
    }

    func testPlannedLedgerMatchesTheDeviceOverviewRows() {
        let control = makeControl()
        let route = control.fixtures.routes[0].summary
        let model = RouteDetailModel(transport: MockTransport(control: control), dressing: .planned(route))

        XCTAssertEqual(model.stats.map(\.key), ["Distance", "Climb", "Est. time", "Max grade"])
        // Assert against the formatter, not an en-US literal: the numbers are locale-aware.
        XCTAssertEqual(model.stats[0].value, OBCFormat.distanceValue(meters: 62_400))
        XCTAssertEqual(model.stats[0].unit, "km")
        XCTAssertEqual(model.stats[2].value, "3:12")  // Road, 62.4 km and 840 m, floored to the minute
        XCTAssertEqual(model.stats[2].unit, "h")
        // Max grade shows an em dash with no grade known.
        XCTAssertEqual(model.stats[3].value, "—")
        XCTAssertNil(model.stats[3].unit)
    }

    /// A route that climbs cannot have a steepest grade of zero or less: that grade comes from
    /// missing elevation, so it reads as unknown. A flat route really is 0 %.
    func testMaxGradeReadsUnknownWhenItContradictsTheClimb() {
        func grade(_ percent: Double, climb: Double) -> String {
            let route = RouteSummary(id: RouteID("g"), name: "G", distanceMeters: 10_000, elevationGainMeters: climb)
            let detail = RouteDetail(summary: route, waypoints: [], elevationProfile: [], maxGradePercent: percent)
            let model = RouteDetailModel(
                transport: MockTransport(control: makeControl()), dressing: .planned(route), preloadedDetail: detail
            )
            return model.stats[3].value
        }
        XCTAssertEqual(grade(8.6, climb: 210), "9")
        XCTAssertEqual(grade(0.2, climb: 210), "—")
        XCTAssertEqual(grade(-1, climb: 0), "0")
    }

    func testConnectionFollowsTheLiveLink() async throws {
        let control = makeControl()
        let route = control.fixtures.routes[0].summary
        let model = RouteDetailModel(transport: MockTransport(control: control), dressing: .planned(route))

        model.start()
        try await waitFor("connected replay", timeout: .seconds(5)) { model.connection == .connected }

        control.connection = .outOfRange
        try await waitFor("link down", timeout: .seconds(5)) { model.connection == .outOfRange }
        control.connection = .connected
        try await waitFor("link up", timeout: .seconds(5)) { model.connection == .connected }
    }

    /// An upload commit pins the assigned id, so a second Upload replaces that object instead of
    /// sending a new one. The state reads up to date until the content moves; a rename out-dates it.
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
        model.recordUploaded(objectID: DeviceObjectID(42), crc32: CRC32.checksum(committed.payload))
        XCTAssertEqual(model.deviceCopyState, .upToDate)
        XCTAssertEqual(model.makeUploadBlob().targetObjectID, DeviceObjectID(42), "a re-upload replaces, never duplicates")

        XCTAssertTrue(model.rename(to: "Blue Mounds (shortcut)"))
        XCTAssertEqual(model.deviceCopyState, .outdated, "a rename out-dates the device copy")
        XCTAssertEqual(model.makeUploadBlob().targetObjectID, DeviceObjectID(42), "…and the update still targets the same object")
    }

    // MARK: Tracked

    func testTrackedDressingShowsItsLedgerTimelineAndHighlights() {
        let control = makeControl()
        let entry = control.fixtures.rides[0]  // Kettle Moraine Loop (ride)
        let ride = entry.summary
        let model = RouteDetailModel(
            transport: MockTransport(control: control), dressing: .tracked(ride), ridePoints: entry.points
        )

        XCTAssertEqual(model.stats.map(\.key), ["Distance", "Moving time", "Avg speed", "Climb", "Descent"])
        XCTAssertEqual(model.stats[1].value, OBCFormat.movingTime(ride.movingTime))
        XCTAssertFalse(model.ink.cased, "a ride has no planned-route casing")
        XCTAssertNotNil(model.subtitle)
        XCTAssertTrue(model.isRenamable)
        XCTAssertTrue(model.timeline == nil && model.highlights.isEmpty, "whole-track work waits for start()")

        model.start()
        let highlights = RideHighlights.compute(entry.ride()).map { OBCFormat.highlight($0) }
        XCTAssertFalse(highlights.isEmpty)
        XCTAssertEqual(model.highlights, highlights)
        XCTAssertEqual(model.timeline?.channels, [.elevation, .speed], "a ride without sensors")
    }

    func testTrackedRideWithoutElevationOrPointsHasNoProfile() {
        let control = makeControl()
        let entry = control.fixtures.rides[0]
        let flat = entry.points.map { RidePoint(timestamp: $0.timestamp, coordinate: $0.coordinate) }
        for points in [flat, []] {
            let model = RouteDetailModel(
                transport: MockTransport(control: control), dressing: .tracked(entry.summary), ridePoints: points
            )
            model.start()
            XCTAssertEqual(model.timeline?.channels, points.isEmpty ? nil : [.speed], "no elevation strip without elevation")
        }
    }

    func testTrackedMapCoordinatesUseTheThreadedGeometryOrFallBackToThePreview() {
        let control = makeControl()
        let ride = control.fixtures.rides[0].summary
        let fullTrack = (0..<500).map { Coordinate(latitude: 47.0 + 0.0001 * Double($0), longitude: 11.0) }
        let points = fullTrack.map { RidePoint(timestamp: ride.date, coordinate: $0) }

        let withGeometry = RouteDetailModel(
            transport: MockTransport(control: control), dressing: .tracked(ride), ridePoints: points
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
        // About 1112 m per step; it rises 5 steps then falls 4, so climb and descent are non-zero.
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
        XCTAssertEqual(model.subtitle, "Imported from Komoot")
        XCTAssertTrue(model.isRenamable, "E1 renames before save")
        XCTAssertEqual(model.waypoints.count, 2)
        XCTAssertEqual(model.elevationProfile.count, 10)
        XCTAssertEqual(model.stats.map(\.key), ["Distance", "Climb", "Est. time", "Max grade"])
        XCTAssertEqual(model.stats[1].value, OBCFormat.climbValue(meters: 50))
        XCTAssertEqual(model.distanceMeters, 9 * 1112.0, accuracy: 20)
    }

    /// A file import names its source; a ride saved as a route is a new route from that ride, with
    /// no file the rider ever picked.
    func testTheLandingCopyFollowsTheSource() {
        let file = RouteDetailModel(
            transport: MockTransport(control: makeControl()),
            dressing: .imported(importedRoute, fileName: "schwarzwald.gpx")
        )
        XCTAssertEqual(file.landingTitle, "Imported route")
        XCTAssertEqual(file.subtitle, "Imported from Komoot")

        let rideDate = Date(timeIntervalSince1970: 1_757_577_600)
        let ride = RouteDetailModel(
            transport: MockTransport(control: makeControl()),
            dressing: .imported(importedRoute, fileName: "Schwarzwald.gpx", source: .ride(rideDate))
        )
        XCTAssertEqual(ride.landingTitle, "New route")
        XCTAssertEqual(ride.subtitle, "From your ride · \(OBCFormat.rideDay(rideDate))")
    }

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

    func testSourceLineFallsBackToTheFileName() {
        var route = importedRoute
        route.creator = "RideWithGPS"
        let model = RouteDetailModel(
            transport: MockTransport(control: makeControl()),
            dressing: .imported(route, fileName: "tour.gpx")
        )
        XCTAssertEqual(model.subtitle, "Imported from tour.gpx")
    }

    func testSourceLineRecognizesGarmin() {
        var route = importedRoute
        route.creator = "Garmin Connect"
        let model = RouteDetailModel(
            transport: MockTransport(control: makeControl()),
            dressing: .imported(route, fileName: "course.tcx")
        )
        XCTAssertEqual(model.subtitle, "Imported from Garmin")
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

        let decoded = try RouteObjectCodec.decode(blob.payload)
        XCTAssertEqual(decoded.name, "Kettle Gravel Day")
        XCTAssertEqual(decoded.waypoints.count, 4)
        XCTAssertLessThan(blob.payload.count, 10_000)
    }

    func testPlannedReuploadTargetsTheDeviceObjectID() {
        // The object id makes the device replace the route in place instead of duplicating it.
        let control = makeControl()
        let model = RouteDetailModel(
            transport: MockTransport(control: control),
            dressing: .planned(control.fixtures.routes[0].summary),
            plannedGeometry: importedRoute,
            deviceObjectID: DeviceObjectID(7)
        )
        XCTAssertEqual(model.makeUploadBlob().targetObjectID, DeviceObjectID(7))
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
        // Uploading saves it too: the device copy and the library copy must be the same route.
        XCTAssertEqual(model.makeUploadBlob().summary.id, model.makeDetail().summary.id)
    }

    func testPreloadedDetailRendersAtOnce() {
        let control = makeControl()
        let transport = MockTransport(control: control)
        // A phone-only id: the mock would throw for it, so the preload must cover it.
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
            transport: transport,
            dressing: .planned(summary),
            preloadedDetail: detail
        )

        XCTAssertEqual(model.waypoints.count, 1, "preloaded waypoints must render immediately")
        XCTAssertEqual(model.elevationProfile, [500, 550, 520])
        XCTAssertEqual(model.maxGradePercent, 6)

        model.start()
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
