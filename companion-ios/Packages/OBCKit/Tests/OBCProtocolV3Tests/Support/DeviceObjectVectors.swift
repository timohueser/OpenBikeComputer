import CryptoKit
import Foundation

/// Locates and parses the checked-in Device Object System v2 vector suite.
///
/// The path is resolved from `#filePath`, exactly as `OBCTransportTests/ProtocolVectorTests.swift`
/// resolves the legacy `specs/vectors/` suite, so `swift test --package-path
/// companion-ios/Packages/OBCKit` (what the `ios-unit` CI job runs) reaches the real fixtures with
/// no resource copying and no chance of testing a stale duplicate.
enum DeviceObjectVectors {
    static let suiteDirectory: URL = {
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<7 { url = url.deletingLastPathComponent() }  // …/Support/<file>.swift → repo root
        return url.appendingPathComponent("specs/vectors/device-object-v2")
    }()

    static let manifestURL = suiteDirectory.appendingPathComponent("manifest.json")

    /// One `manifest.json` row.
    struct Entry: Sendable, Hashable, CustomStringConvertible {
        let name: String
        let file: String
        let sha256: String
        /// Which manifest array it came from.
        let section: String

        var description: String { "\(section)/\(name)" }
        var url: URL { DeviceObjectVectors.suiteDirectory.appendingPathComponent(file) }
    }

    struct Manifest: Sendable {
        let suite: String
        let format: Int
        let wireMajor: Int
        let storageFormat: Int
        let sections: [String: [Entry]]

        var allEntries: [Entry] { sections.keys.sorted().flatMap { sections[$0] ?? [] } }
    }

    static let manifest: Manifest = {
        guard let data = FileManager.default.contents(atPath: manifestURL.path),
            let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else {
            fatalError("device-object-v2 manifest missing at \(manifestURL.path)")
        }
        var sections: [String: [Entry]] = [:]
        for section in ["controls", "streams", "storage", "negative", "transcripts"] {
            let rows = (object[section] as? [[String: Any]]) ?? []
            sections[section] = rows.map {
                Entry(
                    name: $0["name"] as? String ?? "", file: $0["file"] as? String ?? "",
                    sha256: $0["sha256"] as? String ?? "", section: section)
            }
        }
        return Manifest(
            suite: object["suite"] as? String ?? "", format: object["format"] as? Int ?? 0,
            wireMajor: object["wire_major"] as? Int ?? 0,
            storageFormat: object["storage_format"] as? Int ?? 0, sections: sections)
    }()

    static var controls: [Entry] { manifest.sections["controls"] ?? [] }
    static var streams: [Entry] { manifest.sections["streams"] ?? [] }
    static var negatives: [Entry] { manifest.sections["negative"] ?? [] }
    static var transcripts: [Entry] { manifest.sections["transcripts"] ?? [] }

    /// Raw file bytes, for the manifest's own SHA-256 pin.
    static func rawBytes(_ entry: Entry) throws -> Data {
        guard let data = FileManager.default.contents(atPath: entry.url.path) else {
            throw VectorError("fixture \(entry.file) missing at \(entry.url.path)")
        }
        return data
    }

    static func json(_ entry: Entry) throws -> [String: Any] {
        let data = try rawBytes(entry)
        guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
            throw VectorError("fixture \(entry.file) is not a JSON object")
        }
        return object
    }

    static func sha256Hex(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }
}

struct VectorError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

extension String {
    /// The vector contract's raw-byte encoding: lower-case, even-length hexadecimal.
    var hexBytes: [UInt8] {
        get throws {
            guard count % 2 == 0 else { throw VectorError("odd-length hex: \(self)") }
            var out: [UInt8] = []
            out.reserveCapacity(count / 2)
            var index = startIndex
            while index < endIndex {
                let next = self.index(index, offsetBy: 2)
                guard let byte = UInt8(self[index..<next], radix: 16) else {
                    throw VectorError("bad hex: \(self[index..<next])")
                }
                out.append(byte)
                index = next
            }
            return out
        }
    }
}

extension Array where Element == UInt8 {
    var hexString: String { map { String(format: "%02x", $0) }.joined() }
}

/// Records which fixtures the suite actually exercised, so the drift guard can prove that no row of
/// `manifest.json` is silently skipped.
final class ExerciseLog: @unchecked Sendable {
    static let shared = ExerciseLog()
    private let lock = NSLock()
    private var names: Set<String> = []

    func record(_ name: String) {
        lock.lock()
        defer { lock.unlock() }
        names.insert(name)
    }

    func contains(_ name: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return names.contains(name)
    }
}
