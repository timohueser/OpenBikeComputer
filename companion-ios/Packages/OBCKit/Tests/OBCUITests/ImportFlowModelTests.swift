import XCTest
import OBCDomain
import OBCTransport
@testable import OBCUI

/// The import flow's state machine (C3, #357), host-side: decode → E1 vs the
/// name-collision detour, the Replace fingerprint carry-through, the "Add as a
/// new route" rename rules, and the H5 failure paths — driven through an
/// `InMemoryLibraryStore` and a stub decoder (the real `RouteImporter` stays
/// at the app edge; OBCUI never sees OBCFormats).
@MainActor
final class ImportFlowModelTests: XCTestCase {
    private struct StubDecodeError: Error {}

    /// A model over a stub decoder: bytes spelling "bad" fail (H5), anything
    /// else decodes to a route named after the file's stem.
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
        deviceObjectID: UInt16? = nil,
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
            deviceObjectID: deviceObjectID,
            uploadedCRC32: uploadedCRC32
        )
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

    // MARK: Fresh import (→ E1)

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

    /// No bond at arrival → the H4 framing bit is set on the pending import.
    func testUnbondedArrivalFramesThePendingImportAsNoDevicePaired() {
        let model = makeModel(isBonded: false)
        model.open(data: Data("<gpx/>".utf8), fileName: "Alpine Loop.gpx")
        XCTAssertEqual(model.pendingImport?.noDevicePaired, true)
    }

    // MARK: Name collision (→ the update-or-add dialog)

    /// The collision check reads the library **store directly** (a share can
    /// arrive before the launch gate ever built the main screen), keys on the
    /// trimmed lowercased name, and holds E1 until the user picks.
    func testCollidingNameOffersTheDialogInsteadOfOpeningE1() {
        let library = InMemoryLibraryStore()
        let existing = savedRecord(name: "Schwarzwald Tour · Tag 2")
        library.savePlannedRoute(existing)
        let model = makeModel(library: library, decodedName: "  SCHWARZWALD tour · tag 2 ")

        model.open(data: Data("<gpx/>".utf8), fileName: "tag2-v2.gpx")

        XCTAssertNil(model.pendingImport, "E1 must wait for the update-or-add choice")
        XCTAssertEqual(model.collision?.existing.id, existing.id)
    }

    /// Replace: the pending import is pinned to the saved record, and
    /// `record(for:)` carries its `deviceObjectID` + `uploadedCRC32` through —
    /// the device keeps the old copy (old fingerprint → honest "out of date")
    /// until the next push.
    func testReplaceCarriesTheDeviceFingerprintThroughRecordFor() {
        let library = InMemoryLibraryStore()
        let existing = savedRecord(deviceObjectID: 7, uploadedCRC32: 0xDEAD_BEEF)
        library.savePlannedRoute(existing)
        let model = makeModel(library: library, decodedName: "Schwarzwald Tour · Tag 2")

        model.open(data: Data("<gpx2/>".utf8), fileName: "tag2-v2.gpx")
        model.chooseReplace()

        XCTAssertNil(model.collision)
        let pending = model.pendingImport
        XCTAssertEqual(pending?.replacing?.id, existing.id, "the landing reuses the saved id")

        let record = pending!.record(for: detail(named: "Schwarzwald Tour · Tag 2", id: existing.id.rawValue))
        XCTAssertEqual(record.id, existing.id)
        XCTAssertEqual(record.deviceObjectID, 7)
        XCTAssertEqual(record.uploadedCRC32, 0xDEAD_BEEF)
        XCTAssertEqual(record.sourceFileData, Data("<gpx2/>".utf8), "the record keeps the NEW file's bytes")
    }

    /// …but a fresh upload's committed fingerprint wins over the carried one.
    func testRecordForPrefersAJustCommittedFingerprint() {
        let library = InMemoryLibraryStore()
        library.savePlannedRoute(savedRecord(deviceObjectID: 7, uploadedCRC32: 0xDEAD_BEEF))
        let model = makeModel(library: library, decodedName: "Schwarzwald Tour · Tag 2")

        model.open(data: Data("<gpx2/>".utf8), fileName: "tag2-v2.gpx")
        model.chooseReplace()

        let record = model.pendingImport!.record(
            for: detail(named: "Schwarzwald Tour · Tag 2", id: "saved-route"),
            deviceObjectID: 9,
            uploadedCRC32: 0xC0FF_EE00
        )
        XCTAssertEqual(record.deviceObjectID, 9)
        XCTAssertEqual(record.uploadedCRC32, 0xC0FF_EE00)
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

    /// The prompt's validation: empty and still-colliding names are rejected
    /// (a duplicate would just re-collide on the next import).
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

    /// An accepted rename opens E1 as a plain new import: trimmed name on the
    /// route, `replacing` cleared.
    func testAcceptedRenameOpensE1AsAPlainNewImport() {
        let library = InMemoryLibraryStore()
        library.savePlannedRoute(savedRecord(deviceObjectID: 7))
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
        XCTAssertNil(record.deviceObjectID, "no fingerprint rides along without a replace")
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

    // MARK: Failure paths (H5)

    func testUndecodableDataLandsInImportFailed() {
        let model = makeModel()
        model.open(data: Data("bad".utf8), fileName: "notes.txt")
        XCTAssertTrue(model.importFailed)
        XCTAssertNil(model.pendingImport)
        XCTAssertNil(model.collision)
    }

    /// An unreadable URL is the same H5 failure — and the read happens off the
    /// main actor (`openFile` is the async wrapper over `open`).
    func testUnreadableFileLandsInImportFailed() async {
        let model = makeModel()
        let missing = URL(fileURLWithPath: "/nonexistent/\(UUID().uuidString).gpx")
        await model.openFile(at: missing)
        XCTAssertTrue(model.importFailed)
        XCTAssertNil(model.pendingImport)
    }

    /// The happy read: a real temp file flows through `openFile` into E1.
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
