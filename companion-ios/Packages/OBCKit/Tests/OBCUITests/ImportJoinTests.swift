import Testing
import Foundation
import OBCDomain
import OBCTransport
@testable import OBCUI

/// Several files at once: a share arrives one URL at a time and opens as one "Make a trip" sheet;
/// one file still opens the landing.
@MainActor
struct ImportJoinTests {
    private func model(window: Duration = .milliseconds(50)) -> ImportFlowModel {
        ImportFlowModel(
            decode: { data, _ in
                guard data != Data("bad".utf8) else { throw DeviceError.readFailed }
                return ImportedRoute(points: [
                    RoutePoint(coordinate: Coordinate(latitude: 46.5, longitude: 8.0)),
                    RoutePoint(coordinate: Coordinate(latitude: 46.5, longitude: 8.1)),
                ])
            },
            library: InMemoryLibraryStore(), isBonded: { true }, batchWindow: window)
    }

    private func files(_ contents: [String]) throws -> [URL] {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("join-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return try contents.enumerated().map { index, content in
            let url = dir.appendingPathComponent("Stage \(index + 1).gpx")
            try Data(content.utf8).write(to: url)
            return url
        }
    }

    @Test
    func aShareOfSeveralFilesOpensOneJoinSheet() async throws {
        let model = model()
        for url in try files(["a", "b", "c", "d"]) { model.receive(url) }
        for _ in 0..<200 where model.pendingJoin == nil { try await Task.sleep(for: .milliseconds(10)) }
        #expect(model.pendingJoin?.files.map(\.fileName) == ["Stage 1.gpx", "Stage 2.gpx", "Stage 3.gpx", "Stage 4.gpx"])
        #expect(model.pendingImport == nil)
    }

    @Test
    func oneFileStillOpensTheLanding() async throws {
        let model = model()
        await model.openFiles(at: try files(["a"]))
        #expect(model.pendingImport?.fileName == "Stage 1.gpx")
        #expect(model.pendingJoin == nil)
    }

    @Test
    func oneUnreadableFileOpensNothing() async throws {
        let model = model()
        await model.openFiles(at: try files(["a", "bad"]))
        #expect(model.importFailed)
        #expect(model.pendingJoin == nil)
    }
}
