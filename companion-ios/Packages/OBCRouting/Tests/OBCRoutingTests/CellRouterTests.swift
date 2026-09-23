import CryptoKit
import Foundation
import OBCDomain
import Testing
@testable import OBCRouting

private let repo = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
private let fixture = repo.appending(path: "apps/obc-web-assemble/tests/fixture")

/// The vector `apps/obc-companion-core/tests/route.rs` writes from the host router.
private struct Vector: Decodable {
    let from: [Int32]
    let to: [Int32]
    let profile: UInt8
    let distanceM: Int
    let ascentM: Int
    let points: [Row]

    /// `[lon, lat, ele or null, surface, elevation incomplete]`.
    struct Row: Decodable, Equatable {
        let lon: Int
        let lat: Int
        let ele: Int?
        let surface: Int
        let incomplete: Bool

        init(_ point: RoutePoint) {
            lon = Int((point.coordinate.longitude * 1e6).rounded())
            lat = Int((point.coordinate.latitude * 1e6).rounded())
            ele = point.elevationMeters.map { Int($0) }
            surface = Int(point.surface)
            incomplete = point.elevationIncomplete
        }

        init(from decoder: Decoder) throws {
            var row = try decoder.unkeyedContainer()
            lon = try row.decode(Int.self)
            lat = try row.decode(Int.self)
            ele = try row.decodeNil() ? nil : try row.decode(Int.self)
            surface = try row.decode(Int.self)
            incomplete = try row.decode(Bool.self)
        }
    }

    static func load() throws -> Vector {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let url = repo.appending(path: "apps/obc-companion-core/tests/route-vector.json")
        return try decoder.decode(Vector.self, from: Data(contentsOf: url))
    }

    var start: Coordinate { Coordinate(latitude: Double(from[1]) / 1e6, longitude: Double(from[0]) / 1e6) }
    var end: Coordinate { Coordinate(latitude: Double(to[1]) / 1e6, longitude: Double(to[0]) / 1e6) }
}

/// The fixture's network cells and terrain cells published as a catalog on a fake origin, with
/// switches for the failures the router must survive or report.
private actor Server {
    static let catalog = URL(string: "https://cells.test/catalog.json")!

    private(set) var objects: [URL: Data] = [:]
    private(set) var requests: [URL] = []
    var offline = false
    /// Serve these objects cut short once.
    var tear: Set<URL> = []
    /// Serve these objects with one byte changed, every time.
    var corrupt: Set<URL> = []

    init() throws {
        func json(_ name: String) throws -> [String: Any] {
            try JSONSerialization.jsonObject(with: Data(contentsOf: fixture.appending(path: name))) as! [String: Any]
        }
        let cells = try json("cells.json")
        let terrain = try json("terrain.json")
        var objects: [URL: Data] = [:]
        func publish(_ data: Data, _ stem: String, _ ext: String) -> [String: Any] {
            let sha = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
            let url = URL(string: "https://cells.test/\(stem).\(sha).\(ext)")!
            objects[url] = data
            return ["bytes": data.count, "sha256": sha, "url": url.absoluteString]
        }
        func index(_ entries: [[String: Any]], band: String, ext: String) throws -> [String: Any] {
            let published = try entries.map { entry in
                let path = entry["path"] as! String
                let data = try Data(contentsOf: fixture.appending(path: path))
                var pin = publish(data, "cells/\(band)/" + path.split(separator: "/").suffix(2).joined(separator: "/")
                    .replacingOccurrences(of: ".\(ext)", with: ""), ext)
                pin["id"] = entry["id"]
                pin["partial"] = entry["partial"] ?? false
                return pin
            }
            let doc = try JSONSerialization.data(withJSONObject: ["cells": published, "known_empty": []])
            return publish(doc, "cells/\(band)/index", "json")
        }
        let network = (cells["cells"] as! [[String: Any]]).filter { $0["band"] as? String == "network" }
        var networkRef = try index(network, band: "network", ext: "obcm")
        networkRef["band"] = "network"
        let root: [String: Any] = [
            "schema_version": 3,
            "schema": cells["schema"]!,
            "skins": [try json("skin.json")],
            "cell_index": [networkRef],
            "terrain": [
                "posting_log2": terrain["posting_log2"]!,
                "cell_log2": terrain["cell_log2"]!,
                "cell_index": try index(terrain["cells"] as! [[String: Any]], band: "terrain", ext: "obcd"),
            ],
        ]
        objects[Self.catalog] = try JSONSerialization.data(withJSONObject: root)
        self.objects = objects
    }

    func serve(_ url: URL) throws -> Data {
        if offline { throw URLError(.notConnectedToInternet) }
        requests.append(url)
        guard var data = objects[url] else { throw HTTPStatusError(code: 404) }
        if tear.remove(url) != nil { return data.prefix(data.count / 2) }
        if corrupt.contains(url) { data[0] ^= 0xFF }
        return data
    }

    func set(offline: Bool) { self.offline = offline }
    func set(tear: Set<URL>) { self.tear = tear }
    func set(corrupt: Set<URL>) { self.corrupt = corrupt }

    var cellURLs: [URL] { objects.keys.filter { $0.pathExtension == "obcm" }.sorted { $0.absoluteString < $1.absoluteString } }
}

private func scratch() -> CellCache {
    CellCache(directory: FileManager.default.temporaryDirectory.appending(path: "OBCRoutingTests-\(UUID().uuidString)"))
}

private func makeRouter(_ server: Server, _ cache: CellCache) -> CellRouter {
    CellRouter(catalogURL: Server.catalog, cache: cache, fetch: { url in try await server.serve(url) })
}

@Suite struct CellRouterTests {
    @Test func routesLikeTheHostRouter() async throws {
        let vector = try Vector.load()
        let router = makeRouter(try Server(), scratch())

        let leg = try await router.route(from: vector.start, to: vector.end, profile: vector.profile)

        #expect(leg.points.map(Vector.Row.init) == vector.points)
        #expect(leg.distanceMeters == vector.distanceM)
        #expect(leg.ascentMeters == vector.ascentM)
        // Two kilometres south of every way.
        let nowhere = Coordinate(latitude: 47.28, longitude: 7.602176)
        await #expect(throws: RouteFailure.noRoad) {
            try await router.route(from: vector.start, to: nowhere, profile: 0)
        }
    }

    /// Through the app's seam: the first request says it downloads, the second needs no network.
    @Test func aSecondRequestInTheSameAreaIsOffline() async throws {
        let vector = try Vector.load()
        let server = try Server()
        let cache = scratch()
        let downloads = Counter()
        let first = try await makeRouter(server, cache).route(
            from: vector.start, to: vector.end, bikeType: .road, onDownload: downloads.add)
        #expect(await server.requests.count == 6, "the root, two indexes, two network cells, one terrain cell")
        #expect(downloads.value == 5, "every object but the root")

        await server.set(offline: true)
        let again = try await makeRouter(server, cache).route(
            from: vector.start, to: vector.end, bikeType: .road, onDownload: downloads.add)
        #expect(again == first)
        #expect(downloads.value == 5)

        await #expect(throws: LegRouteFailure.noMap, "no cells published here") {
            try await makeRouter(server, cache).route(
                from: Coordinate(latitude: 10, longitude: 10), to: Coordinate(latitude: 10.01, longitude: 10), bikeType: .road) {}
        }

        await #expect(throws: LegRouteFailure.noConnection) {
            try await makeRouter(server, scratch()).route(from: vector.start, to: vector.end, bikeType: .road) {}
        }
    }

    @Test func routesInTheSameAreaShareTheirDownloads() async throws {
        let vector = try Vector.load()
        let server = try Server()
        let router = makeRouter(server, scratch())

        async let out = router.route(from: vector.start, to: vector.end, profile: 0)
        async let back = router.route(from: vector.end, to: vector.start, profile: 0)
        _ = try await (out, back)

        #expect(await server.requests.count == 6)
    }

    @Test func aTornBodyIsFetchedAgainAndAWrongOneIsRefused() async throws {
        let vector = try Vector.load()
        let server = try Server()
        let cell = try #require(await server.cellURLs.first)

        await server.set(tear: [cell])
        _ = try await makeRouter(server, scratch()).route(from: vector.start, to: vector.end, profile: 0)
        #expect(await server.requests.filter { $0 == cell }.count == 2)

        await server.set(corrupt: [cell])
        let failure = await #expect(throws: RouteFailure.self) {
            try await makeRouter(server, scratch()).route(from: vector.start, to: vector.end, profile: 0)
        }
        guard case .cellDownloadFailed = failure else {
            Issue.record("a corrupt cell must fail its download, not \(String(describing: failure))")
            return
        }
    }

    @Test func theCacheEvictsTheLeastRecentlyUsedObjectThatIsNotHeld() throws {
        let cache = CellCache(directory: scratch().directory, capacity: 25)
        let bytes = Data(repeating: 1, count: 10)
        for name in ["a", "b", "c"] {
            _ = try cache.store(bytes, sha256: name)
        }
        #expect(cache.cached("a") != nil)

        try cache.evict(keeping: ["b"])

        #expect(cache.cached("c") == nil)
        #expect(cache.cached("a") != nil)
        #expect(cache.cached("b") != nil)
    }
}

final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0

    var value: Int { lock.withLock { count } }

    func add() { lock.withLock { count += 1 } }
}
