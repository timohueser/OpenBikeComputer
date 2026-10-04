import XCTest
import OBCDomain
import OBCPlanner
import OBCTransport
@testable import OBCUI

/// The import flow's state machine: decode, the keep-or-plan choice, the name-collision detour, the
/// Replace fingerprint carry-through, the "Add as a new route" rename rules, and the failure paths.
/// The real `RouteImporter` stays at the app edge; OBCUI never sees OBCFormats.
@MainActor
final class ImportFlowModelTests: XCTestCase {
    private struct StubDecodeError: Error {}

    /// The file's line, with heights, and one waypoint.
    private static let line = [
        RoutePoint(coordinate: Coordinate(latitude: 48.0, longitude: 8.0), elevationMeters: 500),
        RoutePoint(coordinate: Coordinate(latitude: 48.1, longitude: 8.0), elevationMeters: 550),
        RoutePoint(coordinate: Coordinate(latitude: 48.3, longitude: 8.2), elevationMeters: 600),
    ]
    private static let spring = Waypoint(index: 0, name: "Spring", distanceAlongMeters: 11_000,
                                         coordinate: Coordinate(latitude: 48.1, longitude: 8.0))

    /// A model over a stub decoder: bytes spelling "bad" fail, and anything else decodes to a
    /// route named after the file's stem.
    private func makeModel(library: any LibraryStore = InMemoryLibraryStore(), decodedName: String? = nil) -> ImportFlowModel {
        ImportFlowModel(
            decode: { data, fileName in
                guard data != Data("bad".utf8) else { throw StubDecodeError() }
                return ImportedRoute(name: decodedName ?? (fileName as NSString).deletingPathExtension,
                                     points: Self.line, waypoints: [Self.spring])
            },
            library: library
        )
    }

    /// Opens one file and keeps its line.
    private func keep(_ model: ImportFlowModel, data: String = "<gpx/>", fileName: String = "Alpine Loop.gpx") {
        model.open(files: [(Data(data.utf8), fileName)])
        model.choose(.keepLine)
        model.proceed(using: ShapeSource(shape: .failure(PlannerFailure.unavailable)))
    }

    private func savedRecord(
        name: String = "Schwarzwald Tour · Tag 2", deviceLink: DeviceRouteLink? = nil, uploadedCRC32: UInt32? = nil
    ) -> PlannedRouteRecord {
        PlannedRouteRecord(
            summary: RouteSummary(id: RouteID("saved-route"), name: name, distanceMeters: 88_000, elevationGainMeters: 1_400),
            route: ImportedRoute(name: name, points: [RoutePoint(coordinate: Coordinate(latitude: 48, longitude: 8))]),
            sourceFileName: "tag2.gpx",
            sourceFileData: Data("<gpx/>".utf8),
            deviceLink: deviceLink,
            uploadedCRC32: uploadedCRC32
        )
    }

    private func link(_ objectID: UInt16) -> DeviceRouteLink {
        DeviceRouteLink(serial: "OBC-24-000317", storeID: "0000000000000000000000000000002a", objectID: DeviceObjectID(objectID))
    }

    private func detail(named name: String, id: String) -> RouteDetail {
        RouteDetail(
            summary: RouteSummary(id: RouteID(id), name: name, distanceMeters: 42_000, elevationGainMeters: 800),
            waypoints: [], elevationProfile: [], maxGradePercent: nil
        )
    }

    // MARK: Keep the file's line

    /// The choice comes first; keeping lands the file at once with its line, its heights and a plan
    /// of one drawn leg. The waypoints become markers.
    func testKeptLineLandsWithItsHeightsAndAPlan() throws {
        let model = makeModel()
        model.open(files: [(Data("<gpx/>".utf8), "Alpine Loop.gpx")])
        XCTAssertEqual(model.pendingChoice?.map(\.fileName), ["Alpine Loop.gpx"])
        XCTAssertNil(model.landed, "nothing lands before the choice")

        keep(model)

        let landed = try XCTUnwrap(model.landed)
        XCTAssertNil(model.pendingChoice)
        XCTAssertEqual(landed.route.points, Self.line)
        XCTAssertEqual(landed.fileData, Data("<gpx/>".utf8), "the original bytes ride into the library record")
        let plan = try XCTUnwrap(landed.record(for: detail(named: "Alpine Loop", id: "new")).plan)
        XCTAssertEqual(plan.routePoints.map(\.kind), [.start, .finish])
        XCTAssertEqual(plan.routePoints.last?.leg, .drawn)
        XCTAssertEqual(plan.routePoints.last?.drawn?.last?.elevationMeters, 600, "the drawn leg keeps the heights")
        XCTAssertEqual(plan.markers.map(\.label), ["Spring"])
        XCTAssertEqual(landed.record(for: detail(named: "Alpine Loop", id: "new")).route.waypoints, [Self.spring])
        XCTAssertEqual(plan.bike, RouteActivity(landed.bikeType).rawValue)
    }

    func testCancelingTheChoiceDropsTheImport() {
        let model = makeModel()
        model.open(files: [(Data("<gpx/>".utf8), "Alpine Loop.gpx")])
        model.cancelChoice()
        XCTAssertNil(model.pendingChoice)
        XCTAssertNil(model.landed)
    }

    // MARK: Plan it on roads

    /// The shape points become the plan, and the routed line through them replaces the file's line.
    /// The waypoints move onto the routed line.
    func testPlanOnRoadsLandsTheShapedPlanAndTheRoutedLine() async throws {
        let model = makeModel()
        let shape = PlannedShape(points: [Self.line[0].coordinate, Self.line[2].coordinate, Self.line[0].coordinate], turnarounds: [1])
        let source = ShapeSource(shape: .success(shape))
        model.open(files: [(Data("<gpx/>".utf8), "Alpine Loop.gpx")])

        model.choose(.planOnRoads)
        let planning = model.proceed(using: source)
        XCTAssertEqual(model.planning?.of, 1)
        await planning?.value

        XCTAssertNil(model.planning)
        let landed = try XCTUnwrap(model.landed)
        XCTAssertNil(landed.keptReason)
        XCTAssertEqual(landed.route.points.map(\.elevationMeters), [300, 300, 300], "the routed line")
        let plan = try XCTUnwrap(landed.plan)
        XCTAssertEqual(plan.routePoints.map(\.kind), [.start, .via, .finish])
        XCTAssertEqual(plan.routePoints.map(\.turnaround), [nil, true, nil])
        XCTAssertEqual(plan.markers.map(\.label), ["Spring"])
        XCTAssertEqual(landed.route.waypoints, Waypoint.placed([Self.spring], along: landed.route.points))
        XCTAssertNotEqual(landed.route.waypoints.first?.distanceAlongMeters, Self.spring.distanceAlongMeters)
        let lines = await source.shapeLines
        XCTAssertEqual(lines, [Self.line.map(\.coordinate)])
        let profiles = await source.profilesAsked
        XCTAssertEqual(profiles, [RouteActivity(landed.bikeType).rawValue], "the Balanced profile of the bike type")
    }

    /// Any failure, a busy service too, keeps the file's line and says why in one line.
    func testAFailedShapeKeepsTheLineAndSaysWhy() async throws {
        for (failure, reason) in [(PlannerFailure.lineNotReproducible, "no route on roads follows it"),
                                  (.busy, "the route service is busy")] {
            let model = makeModel()
            model.open(files: [(Data("<gpx/>".utf8), "Alpine Loop.gpx")])
            model.choose(.planOnRoads)
            await model.proceed(using: ShapeSource(shape: .failure(failure)))?.value

            let landed = try XCTUnwrap(model.landed)
            XCTAssertEqual(landed.route.points, Self.line)
            XCTAssertEqual(landed.plan?.routePoints.last?.leg, .drawn)
            XCTAssertEqual(ImportFlowModel.notice(for: [landed]), "Kept the file's line: \(reason).")
        }
    }

    /// Several files get one choice and then the "Make a trip" sheet; the notice names each kept file.
    func testSeveralFilesShareOneChoiceAndOpenTheJoin() async throws {
        let model = makeModel()
        model.open(files: [(Data("a".utf8), "Day 1.gpx"), (Data("b".utf8), "Day 2.gpx")])
        model.choose(.planOnRoads)
        await model.proceed(using: ShapeSource(shape: .failure(PlannerFailure.offlineUnavailable)))?.value

        let files = try XCTUnwrap(model.pendingJoin?.files)
        XCTAssertEqual(files.map(\.fileName), ["Day 1.gpx", "Day 2.gpx"])
        XCTAssertNil(model.landed)
        XCTAssertEqual(ImportFlowModel.notice(for: files),
                       "Kept the line of \u{201C}Day 1.gpx\u{201D}: no connection and no offline map. "
                       + "Kept the line of \u{201C}Day 2.gpx\u{201D}: no connection and no offline map.")
    }

    /// Cancel keeps the files planned so far and the other files' lines, with no further request. A second
    /// import while planning is refused, so no second request runs beside the first.
    func testCancelKeepsWhatIsPlannedAndRefusesASecondImport() async throws {
        let model = makeModel()
        let shape = PlannedShape(points: [Self.line[0].coordinate, Self.line[2].coordinate], turnarounds: [])
        let source = ShapeSource(shape: .success(shape), answers: 1)
        model.open(files: [(Data("a".utf8), "Day 1.gpx"), (Data("b".utf8), "Day 2.gpx"), (Data("c".utf8), "Day 3.gpx")])
        model.choose(.planOnRoads)
        let planning = model.proceed(using: source)
        while await source.shapeLines.count < 2 { try await Task.sleep(for: .milliseconds(5)) }
        XCTAssertEqual(model.planning?.file, 2)

        model.open(files: [(Data("d".utf8), "Day 4.gpx")])
        XCTAssertNil(model.pendingChoice)
        XCTAssertNotNil(model.notice)

        model.cancelPlanning()
        await planning?.value

        let files = try XCTUnwrap(model.pendingJoin?.files)
        XCTAssertEqual(files[0].plan?.routePoints.count, 2)
        XCTAssertNil(files[0].plan?.routePoints.last?.leg, "day 1 is planned")
        XCTAssertEqual(files[1...].map { $0.plan?.routePoints.last?.leg }, [.drawn, .drawn], "the others keep their lines")
        XCTAssertNil(ImportFlowModel.notice(for: files), "a cancel is no failure")
        let requests = await source.shapeLines.count
        XCTAssertEqual(requests, 2, "no request after Cancel")
    }

    // MARK: Name collision (→ the update-or-add dialog)

    /// After the choice, a name match offers the dialog. The check reads the library store directly
    /// and keys on the trimmed, lowercased name.
    func testCollidingNameOffersTheDialogInsteadOfLanding() {
        let library = InMemoryLibraryStore()
        let existing = savedRecord()
        library.savePlannedRoute(existing)
        let model = makeModel(library: library, decodedName: "  SCHWARZWALD tour · tag 2 ")

        keep(model, fileName: "tag2-v2.gpx")

        XCTAssertNil(model.landed, "the route waits for the update-or-add choice")
        XCTAssertEqual(model.collision?.existing.id, existing.id)
    }

    /// Replace pins the import to the saved record, and `record(for:)` carries its `deviceLink` and
    /// `uploadedCRC32` through: the device still holds the old copy. It never mints a new link.
    func testReplaceCarriesTheDeviceFingerprintThroughRecordFor() throws {
        let library = InMemoryLibraryStore()
        let existing = savedRecord(deviceLink: link(7), uploadedCRC32: 0xDEAD_BEEF)
        library.savePlannedRoute(existing)
        let model = makeModel(library: library, decodedName: "Schwarzwald Tour · Tag 2")

        keep(model, data: "<gpx2/>", fileName: "tag2-v2.gpx")
        model.chooseReplace()

        XCTAssertNil(model.collision)
        let landed = try XCTUnwrap(model.landed)
        XCTAssertEqual(landed.replacing?.id, existing.id, "the import reuses the saved id")
        let record = landed.record(for: detail(named: "Schwarzwald Tour · Tag 2", id: existing.id.rawValue))
        XCTAssertEqual(record.id, existing.id)
        XCTAssertEqual(record.deviceLink, link(7))
        XCTAssertEqual(record.uploadedCRC32, 0xDEAD_BEEF)
        XCTAssertEqual(record.sourceFileData, Data("<gpx2/>".utf8), "the record keeps the NEW file's bytes")
        XCTAssertNotNil(record.plan)
    }

    func testCancelingTheCollisionDropsTheImport() {
        let library = InMemoryLibraryStore()
        library.savePlannedRoute(savedRecord())
        let model = makeModel(library: library, decodedName: "Schwarzwald Tour · Tag 2")

        keep(model)
        model.cancelCollision()

        XCTAssertNil(model.collision)
        XCTAssertNil(model.landed)
        XCTAssertNil(model.addAsNewPrompt)
    }

    // MARK: "Add as a new route" (the rename prompt)

    func testAddAsNewNeedsADistinctNameAndLandsAsAPlainNewRoute() throws {
        let library = InMemoryLibraryStore()
        library.savePlannedRoute(savedRecord(deviceLink: link(7)))
        let model = makeModel(library: library, decodedName: "Schwarzwald Tour · Tag 2")

        keep(model)
        model.chooseAddAsNew()
        XCTAssertNil(model.collision)
        XCTAssertNotNil(model.addAsNewPrompt)
        XCTAssertEqual(model.newRouteName, "Schwarzwald Tour · Tag 2")

        XCTAssertFalse(model.isValidNewRouteName("   "))
        XCTAssertFalse(model.isValidNewRouteName("  schwarzwald tour · TAG 2  "), "case/whitespace variants still collide")
        model.confirmNewName(" ")
        XCTAssertNil(model.landed, "an invalid name never lands")

        model.confirmNewName("  Schwarzwald Tour · Tag 3  ")
        XCTAssertNil(model.addAsNewPrompt)
        let landed = try XCTUnwrap(model.landed)
        XCTAssertEqual(landed.route.name, "Schwarzwald Tour · Tag 3")
        XCTAssertNil(landed.replacing, "a renamed add is not a replace")
        XCTAssertNil(landed.record(for: detail(named: "Schwarzwald Tour · Tag 3", id: "new-route")).deviceLink)
    }

    func testCancelingTheRenamePromptDropsTheImport() {
        let library = InMemoryLibraryStore()
        library.savePlannedRoute(savedRecord())
        let model = makeModel(library: library, decodedName: "Schwarzwald Tour · Tag 2")

        keep(model)
        model.chooseAddAsNew()
        model.cancelAddAsNew()

        XCTAssertNil(model.addAsNewPrompt)
        XCTAssertNil(model.landed)
    }

    // MARK: Failure paths

    func testUndecodableDataLandsInImportFailed() {
        let model = makeModel()
        model.open(files: [(Data("bad".utf8), "notes.txt")])
        XCTAssertTrue(model.importFailed)
        XCTAssertNil(model.pendingChoice)
    }

    func testUnreadableFileLandsInImportFailed() async {
        let model = makeModel()
        await model.openFiles(at: [URL(fileURLWithPath: "/nonexistent/\(UUID().uuidString).gpx")])
        XCTAssertTrue(model.importFailed)
        XCTAssertNil(model.pendingChoice)
    }

    func testReadableFileFlowsThroughOpenFilesIntoTheChoice() async throws {
        let model = makeModel()
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("import-flow-\(UUID().uuidString)")
            .appendingPathComponent("Alpine Loop.gpx")
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("<gpx/>".utf8).write(to: url)
        defer { try? FileManager.default.removeItem(at: url.deletingLastPathComponent()) }

        await model.openFiles(at: [url])

        XCTAssertFalse(model.importFailed)
        XCTAssertEqual(model.pendingChoice?.map(\.fileName), ["Alpine Loop.gpx"])
    }
}

/// A shape answer or failure, and a route that is the straight line through the requested points.
/// After `answers` shapes, a shape waits until it is cancelled.
private actor ShapeSource: PlannerDataSource {
    let shape: Result<PlannedShape, Error>
    let answers: Int
    private(set) var shapeLines: [[Coordinate]] = []
    private(set) var profilesAsked: [String] = []
    init(shape: Result<PlannedShape, Error>, answers: Int = .max) { self.shape = shape; self.answers = answers }

    func shape(line: [Coordinate], profile: String) async throws -> PlannedShape {
        shapeLines.append(line); profilesAsked.append(profile)
        if shapeLines.count > answers { try await Task.sleep(for: .seconds(60)) }
        return try shape.get()
    }
    func release() async throws -> PlannerRelease {
        let host = URL(string: "https://planner.test")!
        return PlannerRelease(id: String(repeating: "a", count: 64), region: "test", bounds: [7, 47, 9, 49], basemap: host,
                              glyphs: "", sprites: "", terrain: "", terrain_attribution: "", search: host, routing: host, manifest: host)
    }
    func route(points: [Coordinate], turnarounds: [Int], activity: RouteActivity, preference: RoutePreference,
               release: PlannerRelease) async throws -> PlannedPath {
        let samples = points.map { RoutePoint(coordinate: $0, elevationMeters: 300) }
        let length = MeasuredLine(routePoints: samples).length
        return PlannedPath(points: samples, distance: length, ascent: 0, seconds: length / 4,
                           pointIndices: Array(points.indices), elapsed: points.map { _ in 0 })
    }
    func search(_ query: PlannerSearchQuery, release: PlannerRelease) async throws -> [PlannerPlace] { [] }
}
