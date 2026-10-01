import Foundation
import OBCDomain
import Testing
@testable import OBCPlanner

@Suite("Offline maps")
struct OfflineMapsTests {
    @Test func installationVerifiesAndLinksFilesBeforeMakingThemAvailable() async throws {
        let root = temporary()
        defer { try? FileManager.default.removeItem(at: root) }
        let (quote, bytes) = try fixture()
        let store = OfflineMapStore(root: root)
        try stage(quote, bytes: bytes, root: root)
        try await store.install(quote, allowMobileData: false) { _, _, _ in }
        #expect(try await store.maps() == [quote.map])
        #expect(try await store.pending() == nil)
        let installed = root.appending(path: "releases/\(quote.map.id)/maps/data.json")
        #expect(try Data(contentsOf: installed) == bytes)
        let object = root.appending(path: "objects/\(OfflineBundle.hash(bytes))")
        let attributes = try FileManager.default.attributesOfItem(atPath: object.path)
        #expect((attributes[.referenceCount] as? NSNumber)?.intValue == 2)
        // Reinstall from verified objects needs no transport files or network.
        try await store.install(quote, allowMobileData: false) { _, _, _ in }
        try await store.remove(quote.map)
        #expect(try await store.maps().isEmpty)
        #expect(!FileManager.default.fileExists(atPath: object.path))
    }

    @Test func corruptTransportNeverReplacesAnInstalledMapAndCanBeRetried() async throws {
        let root = temporary()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = OfflineMapStore(root: root)
        let (old, oldBytes) = try fixture(content: "old")
        try stage(old, bytes: oldBytes, root: root)
        try await store.install(old, allowMobileData: false) { _, _, _ in }
        let (next, nextBytes) = try fixture(content: "new")
        try stage(next, bytes: Data("bad".utf8), root: root)
        await #expect(throws: PlannerFailure.invalidData) {
            try await store.install(next, allowMobileData: false) { _, _, _ in }
        }
        #expect(try await store.maps() == [old.map])
        #expect(try await store.pending()?.map == next.map)
        try stage(next, bytes: nextBytes, root: root)
        try await store.install(next, allowMobileData: false) { _, _, _ in }
        #expect(try await store.maps().count == 2)
    }

    @Test func compressedFilesAreVerifiedAndTemporaryCopiesAreRemoved() async throws {
        let root = temporary()
        defer { try? FileManager.default.removeItem(at: root) }
        let (quote, _) = try fixture(content: "hello\n", compressed: true)
        let gzip = Data([31,139,8,0,0,0,0,0,2,3,203,72,205,201,201,231,2,0,32,48,58,54,6,0,0,0])
        try stage(quote, bytes: gzip, root: root)
        let store = OfflineMapStore(root: root)
        try await store.install(quote, allowMobileData: false) { _, _, _ in }
        #expect(try Data(contentsOf: root.appending(path: "releases/\(quote.map.id)/maps/data.json")) == Data("hello\n".utf8))
        #expect(try FileManager.default.contentsOfDirectory(atPath: root.appending(path: "downloads").path).isEmpty)
    }

    @Test func lowStorageBlocksInstallationBeforeItBecomesPending() async throws {
        let root = temporary()
        defer { try? FileManager.default.removeItem(at: root) }
        let (quote, _) = try fixture()
        let store = OfflineMapStore(root: root, capacity: { _ in 0 })
        await #expect(throws: OfflineMapFailure.self) {
            try await store.install(quote, allowMobileData: true) { _, _, _ in }
        }
        #expect(try await store.maps().isEmpty)
        #expect(try await store.pending() == nil)
    }

    @Test(arguments: ["../escape", "/absolute", "maps/../../escape", "release.json"])
    func rejectsUnsafeBundlePaths(_ path: String) throws {
        let (quote, _) = try fixture(path: path)
        #expect(throws: PlannerFailure.invalidData) { try quote.bundle.validate(release: quote.release) }
    }

    @Test func localRequestsDoNotTouchNetworkAndCoverageFallsBack() async throws {
        let root = temporary()
        defer { try? FileManager.default.removeItem(at: root) }
        let (quote, bytes) = try fixture()
        let store = OfflineMapStore(root: root)
        try stage(quote, bytes: bytes, root: root)
        try await store.install(quote, allowMobileData: false) { _, _, _ in }
        let local = RecordingSource(local: true), online = RecordingSource(local: false)
        let planner = LocalFirstPlanner(store: store, online: online) { _, _ in local }
        let release = try await planner.release()
        #expect(release.isLocal)
        let partial = try await planner.mapRelease(bounds: [8,48,10,50], allowNetwork: false)
        #expect(partial.isLocal)
        await #expect(throws: PlannerFailure.offlineUnavailable) {
            try await planner.mapRelease(bounds: [10,50,11,51], allowNetwork: false)
        }
        let points = [Coordinate(latitude: 48, longitude: 8), Coordinate(latitude: 48.1, longitude: 8.1)]
        _ = try await planner.route(points: points, bike: .gravel, preference: .balanced, release: release)
        let results = try await planner.search(.init(text: "absent", view: [8,48,8.1,48.1]), release: release)
        #expect(results.isEmpty)
        #expect(await online.calls.isEmpty)
        #expect(await local.calls == ["release", "release", "release", "route", "release", "search"])
        let far = [points[0], Coordinate(latitude: 50, longitude: 10)]
        _ = try await planner.route(points: far, bike: .gravel, preference: .balanced, release: release)
        #expect(await online.calls == ["release", "route"])
        await online.fail(.unavailable)
        await #expect(throws: PlannerFailure.offlineUnavailable) {
            try await planner.route(points: far, bike: .gravel, preference: .balanced, release: release)
        }
        await local.fail(.noRoad)
        await online.fail(nil)
        _ = try await planner.route(points: points, bike: .gravel, preference: .balanced, release: release)
        #expect(await online.calls.suffix(2) == ["release", "route"])
        await local.cancel()
        let before = await online.calls.count
        await #expect(throws: CancellationError.self) {
            try await planner.route(points: points, bike: .gravel, preference: .balanced, release: release)
        }
        #expect(await online.calls.count == before)
    }

    @Test func viewportSelectionMovesBetweenDownloadsAndOnlineCoverage() async throws {
        let root = temporary()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = OfflineMapStore(root: root)
        let (first, firstBytes) = try fixture(bounds: [7,47,8,48])
        let (second, secondBytes) = try fixture(content: "second", bounds: [9,49,10,50])
        for (quote, bytes) in [(first, firstBytes), (second, secondBytes)] {
            try stage(quote, bytes: bytes, root: root)
            try await store.install(quote, allowMobileData: false) { _, _, _ in }
        }
        let a = RecordingSource(local: true, id: "first", bounds: first.map.bounds)
        let b = RecordingSource(local: true, id: "second", bounds: second.map.bounds)
        let online = RecordingSource(local: false, id: "online", bounds: [7,47,11,51])
        let planner = LocalFirstPlanner(store: store, online: online) { map, _ in map.id == first.map.id ? a : b }
        for (bounds, id) in [([7.2,47.2,7.8,47.8], "first"), ([9.2,49.2,9.8,49.8], "second"),
                             ([10.2,50.2,10.8,50.8], "online")] {
            let selected = try await planner.mapRelease(bounds: bounds)
            #expect(selected.id == id)
            _ = try await planner.overlays(bounds: bounds, zoom: 10, network: "cycling", release: selected)
        }
        #expect(await a.calls.contains("overlays"))
        #expect(await b.calls.contains("overlays"))
        #expect(await online.calls.contains("overlays"))
    }

    @Test func searchRadiusMustFitInsideTheDownload() async throws {
        let root = temporary()
        defer { try? FileManager.default.removeItem(at: root) }
        let (quote, bytes) = try fixture()
        let store = OfflineMapStore(root: root)
        try stage(quote, bytes: bytes, root: root)
        try await store.install(quote, allowMobileData: false) { _, _, _ in }
        let local = RecordingSource(local: true), online = RecordingSource(local: false)
        let planner = LocalFirstPlanner(store: store, online: online) { _, _ in local }
        let release = try await planner.release()
        var query = PlannerSearchQuery(text: "water", view: [8.98,48,8.999,48.01])
        query.kinds = ["water"]; query.alongRoute = true
        query.route = [Coordinate(latitude: 48, longitude: 8.99), Coordinate(latitude: 48.01, longitude: 8.995)]
        for radius: Double? in [nil, 5_000] {
            query.radiusMeters = radius
            _ = try await planner.search(query, release: release)
        }
        query.alongRoute = false; query.radiusMeters = 5_000
        _ = try await planner.search(query, release: release)
        #expect(await online.calls.filter { $0 == "search" }.count == 3)
        #expect(await local.calls.filter { $0 == "search" }.isEmpty)
        query.view = [8,48,8.1,48.1]; query.route = []; query.radiusMeters = 1_000
        _ = try await planner.search(query, release: release)
        #expect(await local.calls.last == "search")
    }
}

private func temporary() -> URL { FileManager.default.temporaryDirectory.appending(path: UUID().uuidString) }
private func fixture(content: String = "map bytes", path: String = "maps/data.json", compressed: Bool = false,
                     bounds: [Double] = [7,47,9,49]) throws -> (OfflineDownloadQuote, Data) {
    let data = Data(content.utf8), file = OfflineFile(bytes: Int64(data.count), sha256: OfflineBundle.hash(data))
    let release = try JSONEncoder().encode(OfflineManifest(format: 1, region: "test", bounds: bounds, files: [path: file]))
    let gzip = Data([31,139,8,0,0,0,0,0,2,3,203,72,205,201,201,231,2,0,32,48,58,54,6,0,0,0])
    let transport = compressed ? gzip : data
    let entry = OfflineBundle.Entry(bytes: file.bytes, sha256: file.sha256,
        transport: .init(bytes: Int64(transport.count), sha256: OfflineBundle.hash(transport), encoding: compressed ? "gzip" : "identity"))
    let bundle = OfflineBundle(format: 1, release: .init(bytes: Int64(release.count), sha256: OfflineBundle.hash(release)), files: [path: entry])
    let map = OfflineMap(id: bundle.release.sha256, name: "Test area", region: "test", bounds: bounds, installedBytes: file.bytes)
    return (.init(map: map, transferBytes: entry.transport.bytes, requiredBytes: 0,
                  source: URL(string: "https://invalid.test/")!, bundle: bundle, release: release), data)
}
private func stage(_ quote: OfflineDownloadQuote, bytes: Data, root: URL) throws {
    let directory = root.appending(path: "downloads")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    try bytes.write(to: directory.appending(path: quote.bundle.objects[0].transport.sha256))
}

private actor RecordingSource: PlannerDataSource {
    var calls: [String] = []
    var failure: PlannerFailure?
    var cancelled = false
    let local: Bool
    let id: String
    let bounds: [Double]
    init(local: Bool, id: String = "test", bounds: [Double] = [7,47,9,49]) {
        self.local = local; self.id = id; self.bounds = bounds
    }
    func fail(_ error: PlannerFailure?) { failure = error }
    func cancel() { cancelled = true }
    func release() throws -> PlannerRelease {
        calls.append("release")
        let url = URL(string: "https://planner.test/")!
        return PlannerRelease(id: id, region: "test", bounds: bounds,
            basemap: local ? URL(string: "pmtiles://file:///map.pmtiles")! : url,
            glyphs: "", sprites: "", terrain: "", terrain_attribution: "", search: url, routing: url, manifest: url)
    }
    func route(points: [Coordinate], bike: BikeType, preference: RoutePreference, release: PlannerRelease) throws -> PlannedPath {
        calls.append("route")
        if cancelled { throw CancellationError() }
        if let failure { throw failure }
        return PlannedPath(points: [], distance: 1, ascent: 0, seconds: 1, pointIndices: [], elapsed: [])
    }
    func search(_ query: PlannerSearchQuery, release: PlannerRelease) -> [PlannerPlace] { calls.append("search"); return [] }
    func overlays(bounds: [Double], zoom: Double, network: String, release: PlannerRelease) -> Data {
        calls.append("overlays"); return Data()
    }
}
