import CryptoKit
import Foundation
import OBCDomain

/// A routed piece: the points the device's router would produce, with elevation where the
/// catalog has terrain.
public struct RoutedLeg: Equatable, Sendable {
    public let points: [RoutePoint]
    public let distanceMeters: Int
    public let ascentMeters: Int
}

public enum RouteFailure: Error, Equatable, Sendable {
    /// The phone is offline and has no cached catalog.
    case noConnection
    /// A catalog object did not arrive, or arrived with the wrong length or digest.
    case cellDownloadFailed(String)
    /// The catalog publishes no road cell near either endpoint.
    case noMap
    /// The catalog or the verified cells could not be read or assembled.
    case mapUnreadable(String)
    /// No road within 100 m of an endpoint.
    case noRoad
    case noPath
    /// The device's node limit filled before the goal: too far to route in one piece.
    case exhausted
}

/// One URL's body. Throws `URLError` for a transport failure and `HTTPStatusError` for a
/// response that is not a success.
public typealias Fetch = @Sendable (URL) async throws -> Data

public struct HTTPStatusError: Error {
    public let code: Int

    public init(code: Int) {
        self.code = code
    }
}

/// Routes on the phone exactly as the device does. For each request it takes the network cells
/// and terrain cells around both endpoints from the published catalog, downloads and verifies the
/// ones not yet cached, assembles them in memory and routes.
///
/// The catalog root is read once per router. A request whose cells are all cached makes no
/// network call after that, and requests running together download each object once.
public actor CellRouter {
    public static let catalogURL = URL(string: "https://maps.openbikecomputer.com/cell-catalog/catalog.json")!

    /// `NAV_MAX_NODES` bounds one route to about R = 10 km of road. Every point P of a route from
    /// A to B of length at most R has |PA| + |PB| ≤ R: an ellipse that lies within R / 2 of the
    /// box spanning A and B. So the cells within R / 2 of that box hold every route the device
    /// could find.
    static let marginMeters = 5_000.0

    private let catalogURL: URL
    private let cache: CellCache
    private let fetch: Fetch
    private var catalog: Task<(text: Data, root: CatalogRoot), Error>?
    private var indexes: [String: CellIndex] = [:]
    private var downloads: [String: Task<URL, Error>] = [:]
    /// How many running requests hold each object; eviction spares them.
    private var held: [String: Int] = [:]

    public init(catalogURL: URL = CellRouter.catalogURL, cache: CellCache = .standard, fetch: @escaping Fetch = CellRouter.download) {
        self.catalogURL = catalogURL
        self.cache = cache
        self.fetch = fetch
    }

    /// Plan from `from` to `to` under the map's nav profile `profile`, the device's bike-type
    /// index. `onDownload` is called before a map object the cache lacks is fetched. Throws a
    /// `RouteFailure`, or `CancellationError` when the task is cancelled.
    public func route(
        from: Coordinate, to: Coordinate, profile: UInt8, onDownload: @escaping @Sendable () -> Void = {}
    ) async throws -> RoutedLeg {
        var mine: [String] = []
        defer { release(mine) }
        let (text, root) = try await loadCatalog()
        guard let core = root.core else { throw RouteFailure.mapUnreadable("the catalog has no core band") }
        var job = Job()

        let cells = CellID.covering(from, to, marginMeters: Self.marginMeters, log2: core.band.cellLog2)
        let index = try await pinnedIndex(core.index, holding: &mine, onDownload: onDownload)
        for cell in cells {
            switch index.lookup(cell) {
            case .artifact(let entry):
                let path = try await object(entry.pin, holding: &mine, onDownload: onDownload).path
                job.cells.append(.init(id: entry.id, band: core.band.id, partial: entry.partial ?? false, path: path))
            case .knownEmpty:
                job.knownEmpty.append(.init(id: cell.id, band: core.band.id))
            case .hole:
                break
            }
        }

        guard !job.cells.isEmpty else { throw RouteFailure.noMap }

        if let terrain = root.terrain {
            let squares = CellID.covering(from, to, marginMeters: Self.marginMeters, log2: terrain.cellLog2)
            let index = try await pinnedIndex(terrain.cellIndex, holding: &mine, onDownload: onDownload)
            for square in squares {
                // A void square has no object and reads as no elevation, so it needs no entry.
                guard case .artifact(let entry) = index.lookup(square) else { continue }
                let path = try await object(entry.pin, holding: &mine, onDownload: onDownload).path
                job.terrain.append(.init(id: entry.id, sha256: entry.sha256, path: path))
            }
        }

        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        let map = try AssembledMap(catalog: text, job: try encoder.encode(job))
        return try map.route(from: from, to: to, profile: profile)
    }

    private func release(_ objects: [String]) {
        for sha256 in objects {
            held[sha256]! -= 1
            if held[sha256] == 0 { held[sha256] = nil }
        }
        // A failed eviction leaves the cache over its capacity until the next request.
        try? cache.evict(keeping: Set(held.keys))
    }

    /// The catalog root, fetched once; a failed fetch is tried again by the next request.
    private func loadCatalog() async throws -> (text: Data, root: CatalogRoot) {
        let task = catalog ?? Task { try await fetchCatalog() }
        catalog = task
        do {
            return try await task.value
        } catch {
            catalog = nil
            throw error
        }
    }

    private func fetchCatalog() async throws -> (text: Data, root: CatalogRoot) {
        let text: Data
        do {
            text = try await withRetry { try await fetch(catalogURL) }
            // Losing this copy loses only the offline fallback.
            try? cache.storeRoot(text)
        } catch let error where Self.isOffline(error) {
            guard let stored = try? Data(contentsOf: cache.root) else { throw RouteFailure.noConnection }
            text = stored
        } catch {
            throw Self.downloadFailure(error, catalogURL)
        }
        let root: CatalogRoot
        do {
            root = try decodeCatalog(CatalogRoot.self, from: text)
        } catch {
            throw RouteFailure.mapUnreadable("the catalog: \(error)")
        }
        guard root.schemaVersion == 3 else {
            throw RouteFailure.mapUnreadable("catalog schema version \(root.schemaVersion) is not 3")
        }
        return (text, root)
    }

    private func pinnedIndex(
        _ pin: Pin, holding mine: inout [String], onDownload: @Sendable () -> Void
    ) async throws -> CellIndex {
        if let index = indexes[pin.sha256] { return index }
        let file = try await object(pin, holding: &mine, onDownload: onDownload)
        do {
            let index = try decodeCatalog(CellIndex.self, from: Data(contentsOf: file))
            indexes[pin.sha256] = index
            return index
        } catch {
            throw RouteFailure.mapUnreadable("\(pin.url): \(error)")
        }
    }

    /// The pinned object's verified file, held for the running request, downloaded only when the
    /// cache lacks it and at most once at a time.
    private func object(
        _ pin: Pin, holding mine: inout [String], onDownload: @Sendable () -> Void
    ) async throws -> URL {
        held[pin.sha256, default: 0] += 1
        mine.append(pin.sha256)
        if let file = cache.cached(pin.sha256) { return file }
        onDownload()
        let task = downloads[pin.sha256] ?? Task { try await fetchObject(pin) }
        downloads[pin.sha256] = task
        defer { downloads[pin.sha256] = nil }
        return try await task.value
    }

    private func fetchObject(_ pin: Pin) async throws -> URL {
        let url = try resolve(pin)
        let data: Data
        do {
            data = try await withRetry { try await verified(url, pin) }
        } catch {
            throw Self.isOffline(error) ? RouteFailure.noConnection : Self.downloadFailure(error, url)
        }
        do {
            return try cache.store(data, sha256: pin.sha256)
        } catch {
            throw RouteFailure.cellDownloadFailed("\(url): the cache refused it: \(error)")
        }
    }

    /// OBCC §9: the object's URL stays on the catalog's origin and carries its digest right
    /// before the final extension.
    private func resolve(_ pin: Pin) throws -> URL {
        guard let url = URL(string: pin.url, relativeTo: catalogURL)?.absoluteURL,
              url.scheme == catalogURL.scheme, url.host == catalogURL.host, url.port == catalogURL.port,
              url.deletingPathExtension().pathExtension == pin.sha256 else {
            throw RouteFailure.cellDownloadFailed("\(pin.url) is not a pinned URL on the catalog's origin")
        }
        return url
    }

    private func verified(_ url: URL, _ pin: Pin) async throws -> Data {
        let data = try await fetch(url)
        if data.count < pin.bytes { throw PinFault.short }
        if data.count > pin.bytes { throw PinFault.long }
        let digest = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        guard digest == pin.sha256 else { throw PinFault.checksum }
        return data
    }

    /// Four attempts, 1.75 s of backoff in all: the CDN now and then drops a connection part-way
    /// through a body, and a second attempt almost always lands. Retrying is safe because every
    /// object is pinned.
    private func withRetry<T>(_ attempt: () async throws -> T) async throws -> T {
        var n = 1
        while true {
            do {
                return try await attempt()
            } catch {
                guard n < 4, Self.worthRetrying(error) else { throw error }
                try await Task.sleep(for: .milliseconds(250 << (n - 1)))
                n += 1
            }
        }
    }

    private static func worthRetrying(_ error: Error) -> Bool {
        switch error {
        case PinFault.short: true
        case is PinFault: false
        case let status as HTTPStatusError: status.code >= 500 || status.code == 408 || status.code == 429
        // A phone with no network at all answers the same way a second later.
        case let error as URLError:
            ![.cancelled, .notConnectedToInternet, .dataNotAllowed, .internationalRoamingOff].contains(error.code)
        default: false
        }
    }

    private static func isOffline(_ error: Error) -> Bool {
        guard let error = error as? URLError else { return false }
        return [.notConnectedToInternet, .networkConnectionLost, .dataNotAllowed, .internationalRoamingOff,
                .cannotFindHost, .cannotConnectToHost, .dnsLookupFailed, .timedOut].contains(error.code)
    }

    private static func downloadFailure(_ error: Error, _ url: URL) -> Error {
        if error is CancellationError { return error }
        if let error = error as? URLError, error.code == .cancelled { return CancellationError() }
        return RouteFailure.cellDownloadFailed("\(url.absoluteString): \(error)")
    }

    /// The production fetch: one GET through the shared session.
    public static let download: Fetch = { url in
        let (data, response) = try await URLSession.shared.data(from: url)
        if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
            throw HTTPStatusError(code: http.statusCode)
        }
        return data
    }
}

enum PinFault: Error {
    case short, long, checksum
}

/// The assembly request `obc_core_assemble` reads.
struct Job: Encodable {
    struct Cell: Encodable {
        let id: String
        let band: String
        let partial: Bool
        let path: String
    }

    struct KnownEmpty: Encodable {
        let id: String
        let band: String
    }

    struct TerrainCell: Encodable {
        let id: String
        let sha256: String
        let path: String
    }

    var cells: [Cell] = []
    var knownEmpty: [KnownEmpty] = []
    var terrain: [TerrainCell] = []
}

extension CellRouter: LegRouter {
    public func route(
        from: Coordinate, to: Coordinate, bikeType: BikeType, onDownload: @escaping @Sendable () -> Void
    ) async throws -> [RoutePoint] {
        do {
            return try await route(from: from, to: to, profile: bikeType.rawValue, onDownload: onDownload).points
        } catch let failure as RouteFailure {
            switch failure {
            case .noConnection: throw LegRouteFailure.noConnection
            case .cellDownloadFailed, .mapUnreadable: throw LegRouteFailure.mapData
            case .noMap, .noRoad, .noPath, .exhausted: throw LegRouteFailure.noRoad
            }
        }
    }
}
