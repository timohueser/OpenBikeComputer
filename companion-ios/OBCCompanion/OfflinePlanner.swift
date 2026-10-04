import Foundation
import OBCPlanner

/// Adapts an installed grid selection to the same client contract as the online planner.
actor OfflinePlanner {
    private let directory: URL
    private let map: OfflineMap
    private let scripts: URL
    private let blocks: OfflineBlocks
    private var router: RouteProvider?
    private var search: PlannerSearchRuntime?

    private init(map: OfflineMap, directory: URL, scripts: URL, blocks: OfflineBlocks) {
        self.map = map; self.directory = directory; self.scripts = scripts; self.blocks = blocks
    }

    static func open(map: OfflineMap, directory: URL,
                     scripts: URL? = Bundle.main.url(forResource: "PlannerSearch", withExtension: nil)) async throws -> any PlannerDataSource {
        guard try plannerFileHash(directory.appending(path: "release.json")) == map.id,
              let scripts else {
            throw PlannerFailure.invalidData
        }
        let manifest = try JSONDecoder().decode(OfflineManifest.self, from: Data(contentsOf: directory.appending(path: "release.json")))
        let blocks = manifest.offline
        try blocks.validate()
        OfflineTilesProtocol.register(id: blocks.id, directory: directory, zoom: blocks.map_zoom)
        let assets = directory.appending(path: "maps/assets").absoluteString
        let release = PlannerRelease(id: map.id, region: map.region, bounds: map.bounds,
            basemap: directory.appending(path: "maps/basemap.json"),
            glyphs: assets + "/fonts/{fontstack}/{range}.pbf", sprites: assets + "/sprites/v4",
            terrain: directory.appending(path: "maps/terrain.json").absoluteString,
            terrain_attribution: manifest.terrain_attribution ?? "",
            search: directory.appending(path: "search"), routing: directory.appending(path: "routing"),
            manifest: directory.appending(path: "release.json"),
            overlays: directory.appending(path: "maps/overlays.json"),
            routes: directory.appending(path: "routes/tiles").absoluteString + "/{cell}.json",
            offlineCells: blocks.cells.map(\.id))
        let runtime = OfflinePlanner(map: map, directory: directory, scripts: scripts, blocks: blocks)
        return PlannerService(release: release) { try await runtime.respond($0) }
    }

    private func respond(_ request: URLRequest) async throws -> (Data, URLResponse) {
        try Task.checkCancellation()
        guard let url = request.url else { throw PlannerFailure.invalidData }
        let response: (status: Int, body: Data)
        switch url.lastPathComponent {
        case "release.json": response = (200, try Data(contentsOf: directory.appending(path: "release.json")))
        case "route", "shape":
            if router == nil { router = try RouteProvider(directory: directory.appending(path: "routing")) }
            let body = request.httpBody ?? Data()
            response = try await url.lastPathComponent == "route" ? router!.route(body) : router!.shape(body)
        case "query":
            if search == nil {
                let names = blocks.cells.flatMap { $0.files.filter { $0.hasPrefix("search/") && $0.hasSuffix(".sqlite") } }
                guard !names.isEmpty else { throw PlannerFailure.invalidData }
                let databases = Array(Set(names)).sorted().map { directory.appending(path: $0) }
                var bounds: [URL: [Double]] = [:]
                for cell in blocks.cells {
                    for name in cell.files where name.hasPrefix("search/") && name.hasSuffix(".sqlite") {
                        bounds[directory.appending(path: name)] = cell.bounds
                    }
                }
                search = try PlannerSearchRuntime(databases: databases, bounds: bounds,
                    scripts: scripts, region: map.region, countryCode: "de", timeZone: "Europe/Berlin")
            }
            response = (200, try search!.request("query", body: request.httpBody ?? Data()))
        default: throw PlannerFailure.invalidData
        }
        try Task.checkCancellation()
        guard let http = HTTPURLResponse(url: url, statusCode: response.status, httpVersion: nil, headerFields: nil) else {
            throw PlannerFailure.invalidData
        }
        return (response.body, http)
    }

}

private struct OfflineManifest: Decodable {
    let offline: OfflineBlocks
    let terrain_attribution: String?
}
private struct OfflineBlocks: Decodable {
    struct Cell: Decodable {
        let id: String
        let bounds: [Double]
        let files: [String]
    }
    let format: Int
    let zoom: Int
    let map_zoom: Int
    let id: String
    let source_routing: String
    let cells: [Cell]
    func validate() throws {
        let hex = CharacterSet(charactersIn: "0123456789abcdef")
        guard format == 2, zoom == 9, map_zoom == 11, [id, source_routing].allSatisfy({ $0.count == 64 && $0.unicodeScalars.allSatisfy(hex.contains) }),
              !cells.isEmpty, Set(cells.map(\.id)).count == cells.count,
              cells.allSatisfy({ $0.id.range(of: "^9-[0-9]+-[0-9]+$", options: .regularExpression) != nil
                  && $0.bounds.count == 4 && $0.bounds.allSatisfy(\.isFinite)
                  && $0.bounds[0] < $0.bounds[2] && $0.bounds[1] < $0.bounds[3]
                  && !$0.files.isEmpty && $0.files.allSatisfy(safeFile) }) else { throw PlannerFailure.invalidData }
    }
}

private func safeFile(_ name: String) -> Bool {
    !name.hasPrefix("/") && !name.contains("\\") && name.split(separator: "/", omittingEmptySubsequences: false)
        .allSatisfy { !$0.isEmpty && $0 != "." && $0 != ".." }
}
