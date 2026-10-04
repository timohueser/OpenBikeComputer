import Foundation

struct PlannerInstallation: Sendable {
    struct File: Decodable, Sendable { let bytes: UInt64; let sha256: String }
    struct Manifest: Decodable, Sendable {
        let format: Int
        let region: String
        let bounds: [Double]
        let files: [String: File]
        let offline: Offline?
        struct Offline: Decodable, Sendable {
            struct Cell: Decodable, Sendable { let bounds: [Double]; let files: [String] }
            let cells: [Cell]
        }
    }
    private struct Active: Decodable { let release: String; let region: String }
    let directory: URL
    let manifest: Manifest

    init(root: URL) throws {
        let active = try JSONDecoder().decode(Active.self, from: Data(contentsOf: root.appendingPathComponent("active.json")))
        guard active.release.range(of: "^[0-9a-f]{64}$", options: .regularExpression) != nil else {
            throw plannerSearchError("Invalid installed release identity")
        }
        directory = root.appendingPathComponent("releases").appendingPathComponent(active.release)
        let file = directory.appendingPathComponent("release.json")
        guard try plannerFileHash(file) == active.release else { throw plannerSearchError("Installed release manifest hash differs") }
        manifest = try JSONDecoder().decode(Manifest.self, from: Data(contentsOf: file))
        guard manifest.format == 1, manifest.region == active.region,
              manifest.region.range(of: "^[a-z][a-z0-9-]{0,63}$", options: .regularExpression) != nil else {
            throw plannerSearchError("Invalid installed release manifest")
        }
    }

    func search(scripts: URL, pythonBundle: URL) throws -> PlannerSearchSession {
        let cells = manifest.offline?.cells ?? []
        let names = Set(cells.isEmpty ? Array(manifest.files.keys) : cells.flatMap(\.files))
            .filter { $0.hasPrefix("search/") && $0.hasSuffix(".sqlite") }.sorted()
        guard !names.isEmpty, names.allSatisfy({ name in
            !name.contains("\\") && name.split(separator: "/", omittingEmptySubsequences: false)
                .allSatisfy { !$0.isEmpty && $0 != "." && $0 != ".." }
        }) else { throw plannerSearchError("Release has no valid search databases") }
        let databases = names.map { directory.appendingPathComponent($0) }
        var hashes: [URL: String] = [:], bounds: [URL: [Double]] = [:]
        for name in names {
            guard let file = manifest.files[name] else { throw plannerSearchError("Search database is absent from release files") }
            hashes[directory.appendingPathComponent(name)] = file.sha256
        }
        for cell in cells {
            for name in cell.files where names.contains(name) { bounds[directory.appendingPathComponent(name)] = cell.bounds }
        }
        var model: [String: String] = [:]
        for name in ["model.int8.onnx", "tokenizer.json", "tokenizer_config.json", "labels.json"] {
            guard let file = manifest.files["search/model/\(name)"] else { throw plannerSearchError("Release has no parser model: \(name)") }
            model[name] = file.sha256
        }
        return try PlannerSearchSession(databases: databases, databaseHashes: hashes, bounds: bounds, coverage: manifest.bounds,
            model: directory.appendingPathComponent("search/model"), modelHashes: model, scripts: scripts,
            region: manifest.region, countryCode: "de", timeZone: "Europe/Berlin", pythonBundle: pythonBundle)
    }
}

extension OfflinePlannerHost {
    static func open(installation: URL, assets: URL, searchScripts: URL, port: UInt16,
                     memoryBudgetBytes: Int = 0) async throws -> OfflinePlannerHost {
        let bundle = Bundle.main.bundleURL
        let (release, api) = try await Task.detached(priority: .userInitiated) {
            let release = try PlannerInstallation(root: installation)
            let search = try release.search(scripts: searchScripts, pythonBundle: bundle)
            let directory = release.directory.appendingPathComponent("routing")
            let router = try RouteProvider(directory: directory, memoryBudgetBytes: memoryBudgetBytes)
            let api = PlannerAPI(search: search, route: { try await router.route($0) },
                region: { try await router.region() },
                sample: try Data(contentsOf: assets.appendingPathComponent("sample.json")))
            return (release, api)
        }.value
        return try OfflinePlannerHost(assets: assets, maps: release.directory.appendingPathComponent("maps"),
            region: release.manifest.region, bounds: release.manifest.bounds, port: port, api: api)
    }
}
