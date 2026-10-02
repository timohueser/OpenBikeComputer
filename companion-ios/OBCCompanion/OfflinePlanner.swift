import Foundation
import OBCPlanner

/// Adapts verified release files to the same client contract as the online planner.
actor OfflinePlanner {
    private let directory: URL
    private let map: OfflineMap
    private let scripts: URL
    private let blocks: OfflineBlocks?
    private let routingPackage: String
    private var router: RouteProvider?
    private var overlays: OverlayProvider?
    private var search: PlannerSearchRuntime?

    private init(map: OfflineMap, directory: URL, scripts: URL, blocks: OfflineBlocks?, routingPackage: String) {
        self.map = map; self.directory = directory; self.scripts = scripts
        self.blocks = blocks; self.routingPackage = routingPackage
    }

    static func open(map: OfflineMap, directory: URL,
                     scripts: URL? = Bundle.main.url(forResource: "PlannerSearch", withExtension: nil)) async throws -> any PlannerDataSource {
        guard try plannerFileHash(directory.appending(path: "release.json")) == map.id,
              let scripts else {
            throw PlannerFailure.invalidData
        }
        let bytes = try Data(contentsOf: directory.appending(path: "release.json"))
        let manifest = try JSONSerialization.jsonObject(with: bytes) as? [String: Any]
        guard let routingPackage = manifest?["routing_package"] as? String else { throw PlannerFailure.invalidData }
        let blocks = try JSONDecoder().decode(OfflineManifest.self, from: bytes).offline
        if let blocks {
            try blocks.validate()
            OfflineTilesProtocol.register(id: blocks.id, directory: directory, zoom: blocks.map_zoom)
        }
        let assets = directory.appending(path: "maps/assets").absoluteString
        let release = PlannerRelease(id: map.id, region: map.region, bounds: map.bounds,
            basemap: blocks == nil ? URL(string: "pmtiles://" + directory.appending(path: "maps/basemap.pmtiles").absoluteString)!
                : directory.appending(path: "maps/basemap.json"),
            glyphs: assets + "/fonts/{fontstack}/{range}.pbf", sprites: assets + "/sprites/v4",
            terrain: blocks == nil ? "pmtiles://" + directory.appending(path: "maps/terrain.pmtiles").absoluteString
                : directory.appending(path: "maps/terrain.json").absoluteString,
            terrain_attribution: manifest?["terrain_attribution"] as? String ?? "",
            search: directory.appending(path: "search"), routing: directory.appending(path: "routing"),
            manifest: directory.appending(path: "release.json"))
        let runtime = OfflinePlanner(map: map, directory: directory, scripts: scripts, blocks: blocks, routingPackage: routingPackage)
        return PlannerService(release: release) { try await runtime.respond($0) }
    }

    private func respond(_ request: URLRequest) async throws -> (Data, URLResponse) {
        try Task.checkCancellation()
        guard let url = request.url else { throw PlannerFailure.invalidData }
        let response: (status: Int, body: Data)
        switch url.lastPathComponent {
        case "release.json": response = (200, try Data(contentsOf: directory.appending(path: "release.json")))
        case "route":
            if router == nil { router = try RouteProvider(directory: directory.appending(path: "routing")) }
            response = try await router!.route(request.httpBody ?? Data())
        case "overlays":
            let query = URLComponents(url: url, resolvingAgainstBaseURL: false)?.percentEncodedQuery ?? ""
            if overlays == nil { overlays = try OverlayProvider(directory: directory.appending(path: "routing")) }
            response = try await overlays!.request(query)
        case "query":
            if search == nil {
                let databases = blocks?.cells.map { directory.appending(path: "search/tiles/\($0.id).sqlite") }
                    ?? [directory.appending(path: "search/\(map.region).sqlite")]
                let bounds = Dictionary(uniqueKeysWithValues: (blocks?.cells ?? []).map {
                    (directory.appending(path: "search/tiles/\($0.id).sqlite"), $0.bounds)
                })
                search = try PlannerSearchRuntime(databases: databases, bounds: bounds, coverage: map.bounds,
                    scripts: scripts, region: map.region, countryCode: "de", timeZone: "Europe/Berlin",
                    parse: { _ in "{\"error\":\"Native search requires a structured request.\"}" })
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

private struct OfflineManifest: Decodable { let offline: OfflineBlocks? }
private struct OfflineBlocks: Decodable {
    struct Cell: Decodable {
        let id: String
        let bounds: [Double]
        func intersects(_ box: [Double]) -> Bool {
            bounds[0] <= box[2] && bounds[2] >= box[0] && bounds[1] <= box[3] && bounds[3] >= box[1]
        }
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
                  && $0.bounds[0] < $0.bounds[2] && $0.bounds[1] < $0.bounds[3] }) else { throw PlannerFailure.invalidData }
    }
}
