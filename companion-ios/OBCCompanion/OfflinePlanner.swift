import Foundation
import OBCPlanner

/// Adapts verified release files to the same client contract as the online planner.
actor OfflinePlanner {
    private let directory: URL
    private let map: OfflineMap
    private let scripts: URL
    private var router: RouteProvider?
    private var overlays: OverlayProvider?
    private var search: PlannerSearchRuntime?

    private init(map: OfflineMap, directory: URL, scripts: URL) {
        self.map = map; self.directory = directory; self.scripts = scripts
    }

    static func open(map: OfflineMap, directory: URL,
                     scripts: URL? = Bundle.main.url(forResource: "PlannerSearch", withExtension: nil)) async throws -> any PlannerDataSource {
        guard try plannerFileHash(directory.appending(path: "release.json")) == map.id,
              let scripts else {
            throw PlannerFailure.invalidData
        }
        let manifest = try JSONSerialization.jsonObject(with: Data(contentsOf: directory.appending(path: "release.json"))) as? [String: Any]
        let assets = directory.appending(path: "maps/assets").absoluteString
        let release = PlannerRelease(id: map.id, region: map.region, bounds: map.bounds,
            basemap: URL(string: "pmtiles://" + directory.appending(path: "maps/basemap.pmtiles").absoluteString)!,
            glyphs: assets + "/fonts/{fontstack}/{range}.pbf", sprites: assets + "/sprites/v4",
            terrain: "pmtiles://" + directory.appending(path: "maps/terrain.pmtiles").absoluteString,
            terrain_attribution: manifest?["terrain_attribution"] as? String ?? "",
            search: directory.appending(path: "search"), routing: directory.appending(path: "routing"),
            manifest: directory.appending(path: "release.json"))
        let runtime = OfflinePlanner(map: map, directory: directory, scripts: scripts)
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
            if overlays == nil { overlays = try OverlayProvider(directory: directory.appending(path: "routing")) }
            response = try await overlays!.request(URLComponents(url: url, resolvingAgainstBaseURL: false)?.percentEncodedQuery ?? "")
        case "query":
            if search == nil {
                search = try PlannerSearchRuntime(database: directory.appending(path: "search/\(map.region).sqlite"),
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
