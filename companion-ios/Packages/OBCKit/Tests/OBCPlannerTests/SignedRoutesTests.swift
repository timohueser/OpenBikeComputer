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
        struct Plan: Decodable { let id: Int; let points_udeg: [[Int]]; let turnarounds: [Int]; let place: [Double]; let nearest_vertex: Int }
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

    @Test func plansTheSharedVector() throws {
        for plan in Self.vector.plans {
            let route = try #require(Self.vector.routes.first { $0.id == plan.id })
            let expected = plan.points_udeg.map { Coordinate(latitude: Double($0[1]) / 1e6, longitude: Double($0[0]) / 1e6) }
            let actual = try #require(route.plan)
            #expect(actual.points == expected && actual.turnarounds == plan.turnarounds)
            let place = Coordinate(latitude: plan.place[1], longitude: plan.place[0])
            #expect(SignedRoutes.nearestVertex(route.line, to: place) == plan.nearest_vertex)
        }
    }

    @Test func readsCoveredCellsFromAnOfflineGridAndARegionFile() async throws {
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
        try write(routes, "routes/test.json")
        // The union of cells 9-267-177 and 9-267-178.
        let bounds = [7.734375, 47.5172006978394, 8.4375, 48.45835188280866]
        func release(_ routes: String) -> PlannerRelease {
            PlannerRelease(id: String(repeating: "a", count: 64), region: "test", bounds: bounds, basemap: directory, glyphs: "",
                           sprites: "", terrain: "", terrain_attribution: "", search: directory, routing: directory,
                           manifest: directory.appending(path: "release.json"), routes: routes)
        }
        let grid = try #require(RouteCatalog(release: release(directory.appending(path: "routes/tiles").absoluteString + "/{cell}.json")))
        #expect(grid.covered == ["9-267-177", "9-267-178"])
        #expect(try await grid.loadCell("9-267-178")?.map(\.id) == [101, 105])
        // A neighbour that only shares an edge with the download is not covered.
        #expect(try await grid.loadCell("9-268-178") == nil)
        await #expect(throws: PlannerFailure.invalidData) { try await grid.loadCell("9-267-177") }
        let region = try #require(RouteCatalog(release: release(directory.appending(path: "routes/test.json").absoluteString)))
        #expect(try await region.loadCell("9-267-177")?.map(\.id) == [105])
        #expect(try await region.loadCell("9-267-178")?.map(\.id) == [101, 105])
    }
}
