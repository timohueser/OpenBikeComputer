import CryptoKit
import Foundation

/// Read-only search cells for the shared JavaScript federation. The owner serializes queries.
/// A connection attaches at most eight cells, below SQLite's default attachment limit; its own
/// schema is private memory.
final class PlannerSearchDatabase {
    private var connections: [PlannerSearchConnection] = []
    private var names: [[String]] = []

    init(files: [URL]) throws {
        guard !files.isEmpty, Set(files).count == files.count else { throw plannerSearchError("Invalid search cell selection") }
        for start in stride(from: 0, to: files.count, by: 8) {
            let connection = try PlannerSearchConnection(":memory:", memory: true)
            var group: [String] = []
            for index in start..<min(start + 8, files.count) {
                var uri = URLComponents(url: files[index].absoluteURL, resolvingAgainstBaseURL: false)
                uri?.queryItems = [URLQueryItem(name: "mode", value: "ro"), URLQueryItem(name: "immutable", value: "1")]
                guard let path = uri?.string else { throw plannerSearchError("Invalid search file") }
                _ = try connection.rows("ATTACH DATABASE ? AS c\(index)", [path])
                _ = try connection.rows("PRAGMA c\(index).cache_size=-\(max(128, 32768 / files.count))")
                _ = try connection.rows("PRAGMA c\(index).mmap_size=0")
                group.append("c\(index)")
            }
            connections.append(connection)
            names.append(group)
        }
    }

    /// The attached cell schemas of each connection, as JSON.
    func groups() -> String {
        String(decoding: (try? JSONSerialization.data(withJSONObject: names)) ?? Data("[]".utf8), as: UTF8.self)
    }

    /// Runs one statement with JSON parameters. Replies `{"rows":[...]}` or `{"error":"..."}`.
    func run(_ group: Int, _ sql: String, _ parameters: String) -> String {
        do {
            guard connections.indices.contains(group),
                  let values = try JSONSerialization.jsonObject(with: Data(parameters.utf8)) as? [Any] else {
                throw plannerSearchError("Invalid search query arguments")
            }
            return try connections[group].json(sql, values)
        } catch {
            let reply = try? JSONSerialization.data(withJSONObject: ["error": error.localizedDescription])
            return String(decoding: reply ?? Data("{\"error\":\"Search failed\"}".utf8), as: UTF8.self)
        }
    }
}

func plannerFileHash(_ url: URL) throws -> String {
    let file = try FileHandle(forReadingFrom: url)
    defer { try? file.close() }
    var hash = SHA256()
    while try autoreleasepool(invoking: { () throws -> Bool in
        guard let data = try file.read(upToCount: 1 << 20), !data.isEmpty else { return false }
        hash.update(data: data)
        return true
    }) {}
    return hash.finalize().map { String(format: "%02x", $0) }.joined()
}
