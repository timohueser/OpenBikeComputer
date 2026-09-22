import Foundation

/// Verified catalog objects on disk, each named by its SHA-256. Past `capacity` bytes the least
/// recently used go first, and a hit counts as a use. The last catalog root is kept beside them
/// and never evicted, so a rider with no signal can still route over the cells on the phone.
public struct CellCache: Sendable {
    /// Chosen by the owner: room for the cells of a few days' riding.
    public static let defaultCapacity = 100 * 1024 * 1024

    public static var standard: CellCache {
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        return CellCache(directory: caches.appending(path: "OBCCells", directoryHint: .isDirectory))
    }

    public let directory: URL
    let capacity: Int

    public init(directory: URL, capacity: Int = CellCache.defaultCapacity) {
        self.directory = directory
        self.capacity = capacity
    }

    var root: URL { directory.appending(path: "catalog.json") }

    /// The stored object with this digest, marked as just used.
    func cached(_ sha256: String) -> URL? {
        let url = directory.appending(path: sha256)
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        // A failed touch only lets the entry age early; the file itself is sound.
        try? FileManager.default.setAttributes([.modificationDate: Date()], ofItemAtPath: url.path)
        return url
    }

    /// Store verified bytes under their digest, then evict down to the capacity.
    func store(_ data: Data, sha256: String) throws -> URL {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appending(path: sha256)
        try data.write(to: url, options: .atomic)
        try evict()
        return url
    }

    func storeRoot(_ data: Data) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try data.write(to: root, options: .atomic)
    }

    private func evict() throws {
        let keys: [URLResourceKey] = [.fileSizeKey, .contentModificationDateKey]
        let files = try FileManager.default
            .contentsOfDirectory(at: directory, includingPropertiesForKeys: keys)
            .filter { $0.lastPathComponent != root.lastPathComponent }
            .map { url in
                let values = try url.resourceValues(forKeys: Set(keys))
                return (url: url, size: values.fileSize ?? 0, used: values.contentModificationDate ?? .distantPast)
            }
            .sorted { $0.used < $1.used }
        var total = files.reduce(0) { $0 + $1.size }
        for file in files where total > capacity {
            try FileManager.default.removeItem(at: file.url)
            total -= file.size
        }
    }
}
