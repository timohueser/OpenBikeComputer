import Foundation
import OBCDomain
import Testing
@testable import OBCPlanner

/// The Swift half of the shared signed-route vector.
struct SignedRoutesTests {
    private struct Vector: Decodable {
        struct Bounds: Decodable { let from: Double?; let to: Double? }
        struct Query: Decodable {
            let start: [Double]
            let radius_km: Double
            let activity: String
            let shape: String
            let distance_km: Bounds?
            let climb_m: Bounds?
            let hardest: [Int]?
            let sort: String
            let covered: [String]?
        }
        struct Expected: Decodable, Equatable { let id: Int; let distance_m: Double }
        struct Case: Decodable { let name: String; let query: Query; let expect: [Expected]; let cells: [String] }
        struct Plan: Decodable { let id: Int; let points_udeg: [[Int]]; let turnarounds: [Int] }
        let grid: [String]
        let routes: [CatalogRecord]
        let queries: [Case]
        let plans: [Plan]
    }

    private actor Asked {
        var cells: [String] = []
        func add(_ cell: String) { cells.append(cell) }
    }

    private static let vector: Vector = {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // OBCPlannerTests
            .deletingLastPathComponent()  // Tests
            .deletingLastPathComponent()  // OBCKit
            .deletingLastPathComponent()  // Packages
            .deletingLastPathComponent()  // companion-ios
            .deletingLastPathComponent()  // repository root
            .appendingPathComponent("specs/vectors/signed-routes.json")
        return try! JSONDecoder().decode(Vector.self, from: Data(contentsOf: url))
    }()

    @Test(arguments: vector.queries.map(\.name))
    func searchesTheSharedVector(_ name: String) async throws {
        let vector = Self.vector, test = try #require(vector.queries.first { $0.name == name }), input = test.query
        let bounds = { (value: Vector.Bounds?) in RouteBounds(from: value?.from, to: value?.to) }
        let query = RouteQuery(start: Coordinate(latitude: input.start[1], longitude: input.start[0]), radiusKm: input.radius_km,
                               activity: try #require(RouteActivity(rawValue: input.activity)),
                               shape: try #require(RouteShape(rawValue: input.shape)),
                               distanceKm: bounds(input.distance_km), climbM: bounds(input.climb_m),
                               hardest: input.hardest.map { $0[0]...$0[1] },
                               sort: try #require(RouteSort(rawValue: input.sort)), covered: input.covered.map(Set.init))
        let asked = Asked()
        let matches = try await SignedRoutes.search(query) { cell in
            await asked.add(cell)
            return vector.grid.contains(cell) ? vector.routes.filter { $0.cells.contains(cell) } : nil
        }
        #expect(await asked.cells.sorted() == test.cells)
        #expect(matches.map { Vector.Expected(id: $0.route.id, distance_m: $0.distanceM.rounded()) } == test.expect)
    }

    @Test func aPlanThatDoesNotFitItsLineIsRejected() throws {
        func record(_ fields: String, loop: Bool = false) throws -> CatalogRecord {
            try JSONDecoder().decode(CatalogRecord.self, from: Data("""
            {"id":1,"kind":"hiking","rank":1,"loop":\(loop),"length_m":1,"ascent_m":0,"descent_m":0,"cells":[],\(fields)}
            """.utf8))
        }
        #expect(try record(#""line_udeg":[0,0,1,1,1,1],"via":[1],"turnarounds":[1]"#).plan != nil)
        #expect(try record(#""line_udeg":[0,0,1,1,1,1],"via":[2]"#).plan == nil)
        #expect(try record(#""line_udeg":[0,0,1,1,1,1],"via":[1],"turnarounds":[2]"#).plan == nil)
        #expect(try record(#""line_udeg":[0,0,1,1,1],"via":[]"#).plan == nil)
        // Only a loop turns back at its start.
        #expect(try record(#""line_udeg":[0,0,1,1,-1,-1],"via":[1],"turnarounds":[0,1]"#, loop: true).plan?.turnarounds == [0, 1])
        #expect(try record(#""line_udeg":[0,0,1,1,1,1],"via":[1],"turnarounds":[0]"#).plan == nil)
    }

    @Test func plansTheSharedVector() throws {
        for plan in Self.vector.plans {
            let route = try #require(Self.vector.routes.first { $0.id == plan.id })
            let expected = plan.points_udeg.map { Coordinate(latitude: Double($0[1]) / 1e6, longitude: Double($0[0]) / 1e6) }
            let actual = try #require(route.plan)
            #expect(actual.points == expected && actual.turnarounds == plan.turnarounds)
        }
    }

    @Test func joinsStagePlansAtSharedEndsUpToTheRequestLimit() {
        let p = (0..<8).map { Coordinate(latitude: 48, longitude: 8 + Double($0) / 100) }
        let joined = RoutePlan.joined([.init(points: [p[0], p[1], p[2]], turnarounds: [1]), .init(points: [p[2], p[3], p[4]], turnarounds: [1]),
                                       .init(points: [p[5], p[6]], turnarounds: [])])
        #expect(joined == RoutePlan(points: [p[0], p[1], p[2], p[3], p[4], p[5], p[6]], turnarounds: [1, 3]))
        let half = (0..<32).map { Coordinate(latitude: 47, longitude: 8 + Double($0) / 100) }
        #expect(RoutePlan.joined([.init(points: half, turnarounds: []), .init(points: half.reversed(), turnarounds: [])])?.points.count == 63)
        #expect(RoutePlan.joined([.init(points: half, turnarounds: []), .init(points: half + p, turnarounds: [])]) == nil)
    }

    @Test func anEmptySearchNamesTheFilterOrTheNextRadius() async throws {
        let vector = Self.vector
        let loader: RouteCellLoader = { cell in vector.grid.contains(cell) ? vector.routes.filter { $0.cells.contains(cell) } : nil }
        let start = Coordinate(latitude: 47.87, longitude: 8.15)
        let mtb = RouteQuery(start: start, radiusKm: 25, activity: .mtb, shape: .any, hardest: 0...1)
        #expect(try await SignedRoutes.hint(mtb, loadCell: loader) == .filter(.hardest))
        // Without its climb bounds, nothing is near either: the next radius with a match is 25 km.
        let touring = RouteQuery(start: start, radiusKm: 10, activity: .touring, shape: .any, climbM: .init(from: 500, to: 800))
        #expect(try await SignedRoutes.hint(touring, loadCell: loader) == .wider(radiusKm: 25, count: 1))
        var far = touring
        far.radiusKm = 50; far.shape = .oneWay
        #expect(try await SignedRoutes.hint(far, loadCell: loader) == nil)
    }

    @Test func readsTheCoveredCellsOfAnOfflineSelectionOrOfTheReleaseBounds() async throws {
        let directory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.createDirectory(at: directory.appending(path: "routes/tiles"), withIntermediateDirectories: true)
        let routes = Self.vector.routes.filter { [101, 105].contains($0.id) }
        func write(_ records: [CatalogRecord], _ path: String) throws {
            let body = try JSONSerialization.data(withJSONObject: ["format": 1, "routes": records.map { ["id": $0.id, "kind": $0.kind.rawValue,
                "rank": $0.rank, "loop": $0.loop, "length_m": $0.length_m, "ascent_m": $0.ascent_m, "descent_m": $0.descent_m, "cells": $0.cells]
            }])
            try body.write(to: directory.appending(path: path))
        }
        try write(routes, "routes/tiles/9-267-178.json")
        // The union of cells 9-267-177 and 9-267-178.
        let bounds = [7.734375, 47.5172006978394, 8.4375, 48.45835188280866]
        func release(_ routes: String, cells: [String]? = nil) -> PlannerRelease {
            PlannerRelease(id: String(repeating: "a", count: 64), region: "test", bounds: bounds, basemap: directory, places: directory, glyphs: "",
                           sprites: "", terrain: "", terrain_attribution: "", search: directory, routing: directory,
                           manifest: directory.appending(path: "release.json"), overlays: directory, routes: routes, offlineCells: cells)
        }
        let tiles = directory.appending(path: "routes/tiles").absoluteString + "/{cell}.json"
        let grid = try #require(RouteCatalog(release: release(tiles, cells: ["9-267-177", "9-267-178"])))
        #expect(grid.covered == ["9-267-177", "9-267-178"])
        #expect(try await grid.loadCell("9-267-178")?.map(\.id) == [101, 105])
        #expect(try await grid.loadCell("9-268-178") == nil)
        await #expect(throws: PlannerFailure.invalidData) { try await grid.loadCell("9-267-177") }
        let online = try #require(RouteCatalog(release: release(tiles)))
        #expect(online.covered == nil)
        // A neighbour that only shares an edge with the bounds is not covered.
        #expect(try await online.loadCell("9-268-178") == nil)
        #expect(try await online.loadCell("9-267-178")?.map(\.id) == [101, 105])
    }
}
