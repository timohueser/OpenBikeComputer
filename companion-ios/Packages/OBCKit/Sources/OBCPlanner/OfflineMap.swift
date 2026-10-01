import CryptoKit
import Foundation

public struct OfflineMap: Codable, Identifiable, Equatable, Sendable {
    public let id: String
    public let name: String
    public let region: String
    public let bounds: [Double]
    public let installedBytes: Int64

    public func contains(_ bounds: [Double]) -> Bool {
        Self.valid(bounds) && Self.valid(self.bounds) && bounds[0] >= self.bounds[0]
            && bounds[1] >= self.bounds[1] && bounds[2] <= self.bounds[2] && bounds[3] <= self.bounds[3]
    }

    public static func valid(_ bounds: [Double]) -> Bool {
        bounds.count == 4 && bounds.allSatisfy(\.isFinite) && bounds[0] < bounds[2] && bounds[1] < bounds[3]
            && bounds[0] >= -180 && bounds[2] <= 180 && bounds[1] >= -85 && bounds[3] <= 85
    }
}

public struct OfflineRegion: Decodable, Identifiable, Sendable {
    public let id: String
    public let name: String
    public let parent: String?
    public let bounds: [Double]
    public let available: Bool
    public let rings: [[[Double]]]

    public func contains(longitude x: Double, latitude y: Double) -> Bool {
        var inside = false
        for ring in rings where ring.count > 2 {
            var previous = ring[ring.count - 1]
            for point in ring {
                guard point.count == 2, previous.count == 2 else { return false }
                if (point[1] > y) != (previous[1] > y),
                   x < (previous[0] - point[0]) * (y - point[1]) / (previous[1] - point[1]) + point[0] {
                    inside.toggle()
                }
                previous = point
            }
        }
        return inside
    }
}

struct OfflineFile: Codable, Equatable, Sendable {
    let bytes: Int64
    let sha256: String
    var valid: Bool { bytes >= 0 && Self.validHash(sha256) }
    static func validHash(_ value: String) -> Bool {
        value.count == 64 && value.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }
}

struct OfflineManifest: Codable, Sendable {
    let format: Int
    let region: String
    let bounds: [Double]
    let files: [String: OfflineFile]
}

struct OfflineBundle: Codable, Sendable {
    struct Entry: Codable, Sendable {
        struct Transport: Codable, Sendable { let bytes: Int64; let sha256: String; let encoding: String }
        let bytes: Int64
        let sha256: String
        let transport: Transport
    }
    let format: Int
    let release: OfflineFile
    let files: [String: Entry]

    var objects: [Entry] {
        var seen: Set<String> = []
        return files.keys.sorted().compactMap { key in
            let entry = files[key]!
            return seen.insert(entry.sha256).inserted ? entry : nil
        }
    }

    func validate(release data: Data) throws -> OfflineManifest {
        guard format == 1, release.valid, Int64(data.count) == release.bytes,
              Self.hash(data) == release.sha256 else { throw PlannerFailure.invalidData }
        let manifest = try JSONDecoder().decode(OfflineManifest.self, from: data)
        guard manifest.format == 1, OfflineMap.valid(manifest.bounds), !manifest.region.isEmpty,
              manifest.region.range(of: "^[a-z][a-z0-9-]{0,63}$", options: .regularExpression) != nil,
              !files.isEmpty, Set(files.keys) == Set(manifest.files.keys) else { throw PlannerFailure.invalidData }
        var content: [String: Int64] = [:], transports: [String: Int64] = [:]
        for (path, entry) in files {
            let parts = path.split(separator: "/", omittingEmptySubsequences: false)
            let file = OfflineFile(bytes: entry.bytes, sha256: entry.sha256)
            let wire = OfflineFile(bytes: entry.transport.bytes, sha256: entry.transport.sha256)
            guard !parts.isEmpty, !parts.contains(where: { $0.isEmpty || $0 == "." || $0 == ".." }),
                  !path.contains("\\"), !path.contains("\0"), path != "release.json", file.valid, wire.valid,
                  manifest.files[path] == file,
                  ["identity", "gzip"].contains(entry.transport.encoding),
                  entry.transport.encoding != "identity" || wire == file else { throw PlannerFailure.invalidData }
            guard content[file.sha256].map({ $0 == file.bytes }) ?? true,
                  transports[wire.sha256].map({ $0 == wire.bytes }) ?? true else { throw PlannerFailure.invalidData }
            content[file.sha256] = file.bytes; transports[wire.sha256] = wire.bytes
        }
        return manifest
    }

    static func hash(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }
}

public struct OfflineDownloadQuote: Codable, Sendable {
    public let map: OfflineMap
    public let transferBytes: Int64
    public let requiredBytes: Int64
    let source: URL
    let bundle: OfflineBundle
    let release: Data
}

public enum OfflineMapFailure: Error, LocalizedError {
    case notEnoughSpace(required: Int64, available: Int64)
    case unavailable(String)
    public var errorDescription: String? {
        switch self {
        case .notEnoughSpace(let required, let available):
            "This map needs \(ByteCountFormatter.string(fromByteCount: required, countStyle: .file)) free. Only \(ByteCountFormatter.string(fromByteCount: available, countStyle: .file)) is available. Choose a smaller area or remove a map."
        case .unavailable(let message): message
        }
    }
}
