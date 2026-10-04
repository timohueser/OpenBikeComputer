import Foundation

/// The route catalog of one release: a file for each covered zoom 9 cell, or one region file.
/// Online the release bounds give the grid cells; an offline grid selection lists its cells.
public actor RouteCatalog {
    public typealias Transport = @Sendable (URLRequest) async throws -> (Data, URLResponse)
    private let source: String
    private let bounds: [Double]
    private let transport: Transport
    private var region: Task<[CatalogRecord], any Error>?
    /// The cells of an offline grid selection: only routes that lie wholly inside them match. Else nil.
    public nonisolated let covered: Set<String>?

    /// Nil when the release has no route catalog.
    public init?(release: PlannerRelease, transport: @escaping Transport = { try await URLSession.shared.data(for: $0) }) {
        guard let routes = release.routes, OfflineMap.valid(release.bounds) else { return nil }
        source = routes; bounds = release.bounds; self.transport = transport
        covered = release.offlineCells.map(Set.init)
    }

    /// The records of the cell `9-X-Y`, or nil for a cell that the release does not cover.
    /// A covered cell without a file is an error, not a cell with no routes.
    public func loadCell(_ id: String) async throws -> [CatalogRecord]? {
        guard covered?.contains(id) ?? Self.covers(bounds, id) else { return nil }
        if source.contains("{cell}") { return try await file(source.replacingOccurrences(of: "{cell}", with: id)) }
        let task = region ?? Task { [source] in try await self.file(source) }
        region = task
        do { return try await task.value.filter { $0.cells.contains(id) } } catch { region = nil; throw error }
    }

    private func file(_ address: String) async throws -> [CatalogRecord] {
        struct File: Decodable { let format: Int; let routes: [CatalogRecord] }
        guard let url = URL(string: address) else { throw PlannerFailure.invalidData }
        let data: Data
        if url.isFileURL {
            guard let bytes = try? Data(contentsOf: url) else { throw PlannerFailure.invalidData }
            data = bytes
        } else {
            var request = URLRequest(url: url)
            request.timeoutInterval = 25
            let (body, response) = try await transport(request)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw PlannerFailure.unavailable }
            data = body
        }
        guard let file = try? JSONDecoder().decode(File.self, from: data), file.format == 1 else { throw PlannerFailure.invalidData }
        return file.routes
    }

    /// Whether the cell overlaps the bounds with a positive area: the grid cells of an online release.
    static func covers(_ bounds: [Double], _ id: String) -> Bool {
        let parts = id.split(separator: "-")
        guard parts.count == 3, parts[0] == "9", let x = Int(parts[1]), let y = Int(parts[2]), (0..<512).contains(x), (0..<512).contains(y) else {
            return false
        }
        let n = 512.0
        func latitude(_ row: Int) -> Double { atan(sinh(.pi * (1 - 2 * Double(row) / n))) * 180 / .pi }
        // The margin keeps a neighbour that only touches the bounds at an edge outside.
        let margin = 1e-9
        return min(bounds[2], Double(x + 1) / n * 360 - 180) - max(bounds[0], Double(x) / n * 360 - 180) > margin
            && min(bounds[3], latitude(y)) - max(bounds[1], latitude(y + 1)) > margin
    }
}
