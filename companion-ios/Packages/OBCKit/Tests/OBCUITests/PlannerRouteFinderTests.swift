import Foundation
import OBCDomain
import OBCPlanner
import Testing
@testable import OBCUI

/// The Routes view state against cell files served from the shared signed-route vector.
@MainActor
struct PlannerRouteFinderTests {
    /// Serves the cells of the vector. Cells with `held` in their ID wait for `release()`; a failing cell answers 500 once.
    private actor Cells {
        /// The vector's records as JSON.
        let routes: Data
        var held: String?
        var failing: String?
        var asked: [String] = []
        private var waiting: [CheckedContinuation<Void, Never>] = []
        init(routes: Data) { self.routes = routes }
        func hold(_ prefix: String?) { held = prefix }
        func fail(_ cell: String?) { failing = cell }
        func release() { held = nil; waiting.forEach { $0.resume() }; waiting = [] }
        func respond(_ request: URLRequest) async throws -> (Data, URLResponse) {
            let cell = request.url!.deletingPathExtension().lastPathComponent
            asked.append(cell)
            if let held, cell.hasPrefix(held) { await withCheckedContinuation { waiting.append($0) } }
            let status = cell == failing ? 500 : 200
            if status == 500 { failing = nil }
            let records = (try JSONSerialization.jsonObject(with: routes) as! [[String: Any]]).filter { ($0["cells"] as? [String])?.contains(cell) == true }
            let body = try JSONSerialization.data(withJSONObject: ["format": 1, "routes": records])
            return (body, HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: nil)!)
        }
    }

    private static let routes: Data = {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("specs/vectors/signed-routes.json")
        let vector = try! JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
        return try! JSONSerialization.data(withJSONObject: vector["routes"]!)
    }()
    private static let near = PlannerRouteStart(coordinate: Coordinate(latitude: 47.87, longitude: 8.15), name: "Titisee")
    /// Its cells hold no mountain bike route.
    private static let far = PlannerRouteStart(coordinate: Coordinate(latitude: 47.87, longitude: 8.9), name: "Far")

    private func finder(_ cells: Cells, offline: [String]? = nil) -> PlannerRouteFinder {
        let host = URL(string: "https://planner.test")!
        let release = PlannerRelease(id: String(repeating: "b", count: 64), region: "test", bounds: [7.7, 47.5, 9.5, 48.5], basemap: host,
                                     glyphs: "", sprites: "", terrain: "", terrain_attribution: "", search: host, routing: host,
                                     manifest: host, routes: "https://planner.test/routes/{cell}.json", offlineCells: offline)
        let finder = PlannerRouteFinder { RouteCatalog(release: $0) { try await cells.respond($0) } }
        finder.use(release)
        return finder
    }

    @Test func aSupersededSearchDropsItsAnswer() async {
        let cells = Cells(routes: Self.routes), finder = finder(cells)
        finder.filters.shape = .any
        finder.start = Self.near
        await cells.hold("9-267-")
        let first = Task { await finder.search(bike: .mtb) }
        while await cells.asked.isEmpty { await Task.yield() }
        finder.start = Self.far
        let second = Task { await finder.search(bike: .mtb) }
        while await !cells.asked.contains(where: { $0.hasPrefix("9-268-") }) { await Task.yield() }
        await cells.release()
        await first.value; await second.value
        #expect(finder.status == .ready && finder.matches.isEmpty)
    }

    @Test func aFailedCellLoadsAgainOnRetry() async {
        let cells = Cells(routes: Self.routes), finder = finder(cells)
        finder.filters.shape = .any
        finder.start = Self.near
        await cells.fail("9-267-178")
        await finder.search(bike: .mtb)
        #expect(finder.status == .failed)
        await finder.search(bike: .mtb)
        #expect(finder.status == .ready && finder.matches.map(\.route.id) == [201])
    }

    @Test func offlineListsOnlyRoutesInsideTheDownload() async {
        let cells = Cells(routes: Self.routes), finder = finder(cells, offline: ["9-267-178"])
        finder.filters.radiusKm = 25
        finder.filters.shape = .any
        finder.start = Self.near
        await finder.search(bike: .touring)
        // Route 301 also crosses 9-267-177, which is not downloaded.
        #expect(finder.offline && finder.status == .ready && finder.matches.isEmpty)
        #expect(Set(await cells.asked) == ["9-267-178"])
    }
}
