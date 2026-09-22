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
/// and terrain cells within the router's reach of both endpoints from the published catalog,
/// downloads and verifies the ones not yet cached, assembles them in memory and routes.
///
/// The catalog root is read once per router. A request whose cells are all cached makes no
/// network call after that.
public actor CellRouter {
    public static let catalogURL = URL(string: "https://maps.openbikecomputer.com/cell-catalog/catalog.json")!

    /// `NAV_MAX_NODES` bounds one search to roughly this far from an endpoint.
    static let reachMeters = 10_000.0

    private let catalogURL: URL
    private let cache: CellCache
    private let fetch: Fetch
    private var catalog: (text: Data, root: CatalogRoot)?
    private var indexes: [String: CellIndex] = [:]

    public init(catalogURL: URL = CellRouter.catalogURL, cache: CellCache = .standard, fetch: @escaping Fetch = CellRouter.download) {
        self.catalogURL = catalogURL
        self.cache = cache
        self.fetch = fetch
    }

    /// Plan from `from` to `to` under the map's nav profile `profile`, the device's bike-type
    /// index. Throws a `RouteFailure`, or `CancellationError` when the task is cancelled.
    public func route(from: Coordinate, to: Coordinate, profile: UInt8) async throws -> RoutedLeg {
        let (text, root) = try await loadCatalog()
        guard let core = root.core else { throw RouteFailure.mapUnreadable("the catalog has no core band") }
        var job = Job()

        let cells = Set([from, to].flatMap { CellID.around($0, radiusMeters: Self.reachMeters, log2: core.band.cellLog2) })
        let index = try await pinnedIndex(core.index)
        for cell in cells.sorted() {
            switch index.lookup(cell) {
            case .artifact(let entry):
                let path = try await object(entry.pin).path
                job.cells.append(.init(id: entry.id, band: core.band.id, partial: entry.partial ?? false, path: path))
            case .knownEmpty:
                job.knownEmpty.append(.init(id: cell.id, band: core.band.id))
            case .hole:
                break
            }
        }

        guard !job.cells.isEmpty else { throw RouteFailure.noMap }

        if let terrain = root.terrain {
            let squares = Set([from, to].flatMap { CellID.around($0, radiusMeters: Self.reachMeters, log2: terrain.cellLog2) })
            let index = try await pinnedIndex(terrain.cellIndex)
            for square in squares.sorted() {
                // A void square has no object and reads as no elevation, so it needs no entry.
                guard case .artifact(let entry) = index.lookup(square) else { continue }
                let path = try await object(entry.pin).path
                job.terrain.append(.init(id: entry.id, sha256: entry.sha256, path: path))
            }
        }

        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        let map = try AssembledMap(catalog: text, job: try encoder.encode(job))
        return try map.route(from: from, to: to, profile: profile)
    }

    private func loadCatalog() async throws -> (text: Data, root: CatalogRoot) {
        if let catalog { return catalog }
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
        catalog = (text, root)
        return (text, root)
    }

    private func pinnedIndex(_ pin: Pin) async throws -> CellIndex {
        if let index = indexes[pin.sha256] { return index }
        let file = try await object(pin)
        do {
            let index = try decodeCatalog(CellIndex.self, from: Data(contentsOf: file))
            indexes[pin.sha256] = index
            return index
        } catch {
            throw RouteFailure.mapUnreadable("\(pin.url): \(error)")
        }
    }

    /// The pinned object's verified file, downloaded only when the cache lacks it.
    private func object(_ pin: Pin) async throws -> URL {
        if let file = cache.cached(pin.sha256) { return file }
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
