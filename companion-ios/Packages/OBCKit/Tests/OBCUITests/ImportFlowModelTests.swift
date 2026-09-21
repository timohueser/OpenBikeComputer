import XCTest
import OBCDomain
import OBCTransport
@testable import OBCUI

/// The import flow's state machine: decode, the name-collision detour, the Replace fingerprint
/// carry-through, the "Add as a new route" rename rules, and the failure paths. The real
/// `RouteImporter` stays at the app edge; OBCUI never sees OBCFormats.
@MainActor
final class ImportFlowModelTests: XCTestCase {
    private struct StubDecodeError: Error {}

    /// A model over a stub decoder: bytes spelling "bad" fail, and anything else decodes to a
    /// route named after the file's stem.
    private func makeModel(
        library: any LibraryStore = InMemoryLibraryStore(),
        isBonded: Bool = true,
        decodedName: String? = nil
    ) -> ImportFlowModel {
        ImportFlowModel(
            decode: { data, fileName in
                guard data != Data("bad".utf8) else { throw StubDecodeError() }
                return ImportedRoute(
                    name: decodedName ?? (fileName as NSString).deletingPathExtension,
                    points: [
                        RoutePoint(coordinate: Coordinate(latitude: 48.0, longitude: 8.0), elevationMeters: 500),
                        RoutePoint(coordinate: Coordinate(latitude: 48.3, longitude: 8.2), elevationMeters: 600),
                    ]
                )
            },
            library: library,
            isBonded: { isBonded }
        )
    }

    private func savedRecord(
        id: String = "saved-route",
        name: String = "Schwarzwald Tour · Tag 2",
        deviceLink: DeviceRouteLink? = nil,
        uploadedCRC32: UInt32? = nil
    ) -> PlannedRouteRecord {
        PlannedRouteRecord(
            summary: RouteSummary(
                id: RouteID(id), name: name,
                distanceMeters: 88_000, elevationGainMeters: 1_400
            ),
            route: ImportedRoute(
                name: name,
                points: [RoutePoint(coordinate: Coordinate(latitude: 48, longitude: 8))]
            ),
            sourceFileName: "tag2.gpx",
            sourceFileData: Data("<gpx/>".utf8),
            deviceLink: deviceLink,
            uploadedCRC32: uploadedCRC32
        )
    }

    /// A scoped device link for the replace-import tests.
    private func link(_ objectID: UInt16) -> DeviceRouteLink {
        DeviceRouteLink(serial: "OBC-24-000317", storeID: "0000000000000000000000000000002a", objectID: DeviceObjectID(objectID))
    }

    private func detail(named name: String, id: String) -> RouteDetail {
        RouteDetail(
            summary: RouteSummary(
                id: RouteID(id), name: name,
                distanceMeters: 42_000, elevationGainMeters: 800
            ),
            waypoints: [],
            elevationProfile: [],
            maxGradePercent: nil
        )
    }

    // MARK: Fresh import

    func testFreshImportOpensThePendingCover() {
        let model = makeModel()
        model.open(data: Data("<gpx/>".utf8), fileName: "Alpine Loop.gpx")

        XCTAssertNil(model.collision)
        XCTAssertFalse(model.importFailed)
        let pending = model.pendingImport
        XCTAssertEqual(pending?.route.name, "Alpine Loop")
        XCTAssertEqual(pending?.fileName, "Alpine Loop.gpx")
        XCTAssertEqual(pending?.fileData, Data("<gpx/>".utf8), "the original bytes ride into the library record")
        XCTAssertEqual(pending?.noDevicePaired, false)
        XCTAssertNil(pending?.replacing)
    }

    func testUnbondedArrivalFramesThePendingImportAsNoDevicePaired() {
        let model = makeModel(isBonded: false)
        model.open(data: Data("<gpx/>".utf8), fileName: "Alpine Loop.gpx")
        XCTAssertEqual(model.pendingImport?.noDevicePaired, true)
    }

    // MARK: Name collision (→ the update-or-add dialog)

    /// The collision check reads the library store directly: a share can arrive before the launch
    /// gate has built the main screen. It keys on the trimmed, lowercased name.
    func testCollidingNameOffersTheDialogInsteadOfOpeningE1() {
        let library = InMemoryLibraryStore()
        let existing = savedRecord(name: "Schwarzwald Tour · Tag 2")
        library.savePlannedRoute(existing)
        let model = makeModel(library: library, decodedName: "  SCHWARZWALD tour · tag 2 ")

        model.open(data: Data("<gpx/>".utf8), fileName: "tag2-v2.gpx")

        XCTAssertNil(model.pendingImport, "E1 must wait for the update-or-add choice")
        XCTAssertEqual(model.collision?.existing.id, existing.id)
    }

    /// Replace pins the pending import to the saved record, and `record(for:)` carries its
    /// `deviceLink` and `uploadedCRC32` through: the device still holds the old copy, so the old
    /// fingerprint keeps the state honestly out of date. `record(for:)` never mints a new link.
    func testReplaceCarriesTheDeviceFingerprintThroughRecordFor() {
        let library = InMemoryLibraryStore()
        let existing = savedRecord(deviceLink: link(7), uploadedCRC32: 0xDEAD_BEEF)
        library.savePlannedRoute(existing)
        let model = makeModel(library: library, decodedName: "Schwarzwald Tour · Tag 2")

        model.open(data: Data("<gpx2/>".utf8), fileName: "tag2-v2.gpx")
        model.chooseReplace()

        XCTAssertNil(model.collision)
        let pending = model.pendingImport
        XCTAssertEqual(pending?.replacing?.id, existing.id, "the landing reuses the saved id")

        let record = pending!.record(for: detail(named: "Schwarzwald Tour · Tag 2", id: existing.id.rawValue))
        XCTAssertEqual(record.id, existing.id)
        XCTAssertEqual(record.deviceLink, link(7))
        XCTAssertEqual(record.uploadedCRC32, 0xDEAD_BEEF)
        XCTAssertEqual(record.sourceFileData, Data("<gpx2/>".utf8), "the record keeps the NEW file's bytes")
    }

    func testCancelingTheCollisionDropsTheImport() {
        let library = InMemoryLibraryStore()
        library.savePlannedRoute(savedRecord())
        let model = makeModel(library: library, decodedName: "Schwarzwald Tour · Tag 2")

        model.open(data: Data("<gpx/>".utf8), fileName: "tag2.gpx")
        model.cancelCollision()

        XCTAssertNil(model.collision)
        XCTAssertNil(model.pendingImport)
        XCTAssertNil(model.addAsNewPrompt)
    }

    // MARK: "Add as a new route" (the rename prompt)

    func testAddAsNewOpensThePromptSeededWithTheCollidingName() {
        let library = InMemoryLibraryStore()
        library.savePlannedRoute(savedRecord())
        let model = makeModel(library: library, decodedName: "Schwarzwald Tour · Tag 2")

        model.open(data: Data("<gpx/>".utf8), fileName: "tag2.gpx")
        model.chooseAddAsNew()

        XCTAssertNil(model.collision)
        XCTAssertNil(model.pendingImport, "E1 must wait for a distinct name")
        XCTAssertNotNil(model.addAsNewPrompt)
        XCTAssertEqual(model.newRouteName, "Schwarzwald Tour · Tag 2")
    }

    func testNewNameValidationRejectsEmptyAndStillCollidingNames() {
        let library = InMemoryLibraryStore()
        library.savePlannedRoute(savedRecord())
        let model = makeModel(library: library, decodedName: "Schwarzwald Tour · Tag 2")

        model.open(data: Data("<gpx/>".utf8), fileName: "tag2.gpx")
        model.chooseAddAsNew()

        model.newRouteName = "   "
        XCTAssertFalse(model.isNewRouteNameValid)
        model.newRouteName = "  schwarzwald tour · TAG 2  "
        XCTAssertFalse(model.isNewRouteNameValid, "case/whitespace variants still collide")
        model.newRouteName = "Schwarzwald Tour · Tag 3"
        XCTAssertTrue(model.isNewRouteNameValid)

        // An invalid name never opens E1, even if confirm is forced.
        model.newRouteName = " "
        model.confirmNewName()
        XCTAssertNil(model.pendingImport)
        XCTAssertNotNil(model.addAsNewPrompt)
    }

    func testAcceptedRenameOpensE1AsAPlainNewImport() {
        let library = InMemoryLibraryStore()
        library.savePlannedRoute(savedRecord(deviceLink: link(7)))
        let model = makeModel(library: library, decodedName: "Schwarzwald Tour · Tag 2")

        model.open(data: Data("<gpx/>".utf8), fileName: "tag2.gpx")
        model.chooseAddAsNew()
        model.newRouteName = "  Schwarzwald Tour · Tag 3  "
        model.confirmNewName()

        XCTAssertNil(model.addAsNewPrompt)
        let pending = model.pendingImport
        XCTAssertEqual(pending?.route.name, "Schwarzwald Tour · Tag 3")
        XCTAssertNil(pending?.replacing, "a renamed add is not a replace")
        let record = pending!.record(for: detail(named: "Schwarzwald Tour · Tag 3", id: "new-route"))
        XCTAssertNil(record.deviceLink, "no fingerprint rides along without a replace")
    }

    func testCancelingTheRenamePromptDropsTheImport() {
        let library = InMemoryLibraryStore()
        library.savePlannedRoute(savedRecord())
        let model = makeModel(library: library, decodedName: "Schwarzwald Tour · Tag 2")

        model.open(data: Data("<gpx/>".utf8), fileName: "tag2.gpx")
        model.chooseAddAsNew()
        model.cancelAddAsNew()

        XCTAssertNil(model.addAsNewPrompt)
        XCTAssertNil(model.pendingImport)
    }

    // MARK: Failure paths

    func testUndecodableDataLandsInImportFailed() {
        let model = makeModel()
        model.open(data: Data("bad".utf8), fileName: "notes.txt")
        XCTAssertTrue(model.importFailed)
        XCTAssertNil(model.pendingImport)
        XCTAssertNil(model.collision)
    }

    /// `openFile` is the async wrapper over `open`; an unreadable URL is the same failure.
    func testUnreadableFileLandsInImportFailed() async {
        let model = makeModel()
        let missing = URL(fileURLWithPath: "/nonexistent/\(UUID().uuidString).gpx")
        await model.openFile(at: missing)
        XCTAssertTrue(model.importFailed)
        XCTAssertNil(model.pendingImport)
    }

    func testReadableFileFlowsThroughOpenFileIntoE1() async throws {
        let model = makeModel()
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("import-flow-\(UUID().uuidString)")
            .appendingPathComponent("Alpine Loop.gpx")
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("<gpx/>".utf8).write(to: url)
        defer { try? FileManager.default.removeItem(at: url.deletingLastPathComponent()) }

        await model.openFile(at: url)

        XCTAssertFalse(model.importFailed)
        XCTAssertEqual(model.pendingImport?.fileName, "Alpine Loop.gpx")
    }

    // MARK: Closing the cover

    func testCloseImportClearsThePendingCover() {
        let model = makeModel()
        model.open(data: Data("<gpx/>".utf8), fileName: "Alpine Loop.gpx")
        model.closeImport()
        XCTAssertNil(model.pendingImport)
    }
}
