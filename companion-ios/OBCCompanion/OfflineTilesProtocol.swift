import Foundation
import MapLibre

/// MapLibre's tile requests read installed archives through URLSession, without a network socket.
final class OfflineTilesProtocol: URLProtocol, @unchecked Sendable {
    private final class Locations: @unchecked Sendable {
        let lock = NSLock()
        var roots: [String: (URL, Int)] = [:]
        var archives: [(URL, PMTilesArchive)] = []
        func set(_ id: String, _ root: URL, _ zoom: Int) { lock.lock(); defer { lock.unlock() }; roots[id] = (root, zoom) }
        func get(_ id: String) -> (URL, Int)? { lock.lock(); defer { lock.unlock() }; return roots[id] }
        func archive(_ file: URL) throws -> PMTilesArchive {
            lock.lock(); defer { lock.unlock() }
            if let at = archives.firstIndex(where: { $0.0 == file }) {
                let cached = archives.remove(at: at); archives.append(cached); return cached.1
            }
            let archive = try PMTilesArchive(file)
            archives.append((file, archive))
            if archives.count > 16 { archives.removeFirst() }
            return archive
        }
    }
    private static let locations = Locations()
    private let lock = NSLock()
    private var cancelled = false

    @MainActor static func install() {
        let configuration = URLSessionConfiguration.default
        configuration.protocolClasses = [OfflineTilesProtocol.self] + (configuration.protocolClasses ?? [])
        MLNNetworkConfiguration.sharedManager.sessionConfiguration = configuration
    }
    static func register(id: String, directory: URL, zoom: Int) { locations.set(id, directory, zoom) }
    override class func canInit(with request: URLRequest) -> Bool { request.url?.host == "offline.openbikecomputer.invalid" }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        do {
            let data = try Self.tile(request)
            lock.lock(); let stopped = cancelled; lock.unlock()
            guard !stopped, let url = request.url else { return }
            let response = HTTPURLResponse(url: url, statusCode: data == nil ? 404 : 200, httpVersion: nil,
                headerFields: ["Content-Type": url.pathComponents.contains("terrain") ? "image/webp" : "application/x-protobuf"])!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            if let data { client?.urlProtocol(self, didLoad: data) }
            client?.urlProtocolDidFinishLoading(self)
        } catch {
            lock.lock(); let stopped = cancelled; lock.unlock()
            if !stopped { client?.urlProtocol(self, didFailWithError: error) }
        }
    }
    override func stopLoading() { lock.lock(); cancelled = true; lock.unlock() }

    private static func tile(_ request: URLRequest) throws -> Data? {
        guard let url = request.url else { throw URLError(.badURL) }
        let parts = url.path.split(separator: "/").map(String.init)
        guard parts.count == 5, ["basemap", "places", "overlays", "terrain"].contains(parts[1]),
              let z = Int(parts[2]), (0...22).contains(z), let x = Int(parts[3]), let y = Int(parts[4]),
              (0..<(1 << z)).contains(x), (0..<(1 << z)).contains(y), let (root, zoom) = locations.get(parts[0]) else {
            throw URLError(.fileDoesNotExist)
        }
        let level = min(z, zoom)
        let file = root.appending(path: "maps/tiles/\(parts[1])/\(level)-\(x >> (z-level))-\(y >> (z-level)).pmtiles")
        guard FileManager.default.fileExists(atPath: file.path) else { return nil }
        return try locations.archive(file).tile(z: z, x: x, y: y)
    }
}
