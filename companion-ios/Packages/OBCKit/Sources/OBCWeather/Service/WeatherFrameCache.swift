import Foundation

/// The cache key of one cropped frame: an immutable object key plus the exact cell window that was
/// cropped out of it.
///
/// Both halves matter. The object key is immutable by the WX5 publishing contract, which is why a
/// cached frame is **never revalidated** — the bytes behind `wx/v2/20260809T1130Z/f15/s2-1.obcg`
/// cannot change, so a conditional request against them would be pure latency. The window makes the
/// entry answer only the question it was stored for; a wider corridor is a miss, not a wrong answer.
public struct WeatherFrameCacheKey: Hashable, Sendable {
    public var objectKey: String
    public var columnMinimum: UInt32
    public var rowMinimum: UInt32
    public var width: Int
    public var height: Int

    public init(objectKey: String, columnMinimum: UInt32, rowMinimum: UInt32, width: Int, height: Int) {
        self.objectKey = objectKey
        self.columnMinimum = columnMinimum
        self.rowMinimum = rowMinimum
        self.width = width
        self.height = height
    }

    public var identifier: String {
        "\(objectKey)#\(columnMinimum),\(rowMinimum),\(width),\(height)"
    }
}

public protocol WeatherFrameCache: Sendable {
    func crop(for key: WeatherFrameCacheKey) async -> PrecipitationCrop?
    func store(_ crop: PrecipitationCrop, for key: WeatherFrameCacheKey) async
}

/// Process-lifetime cache. Enough on its own for one weather job and for the tests; the file cache
/// below is what survives the app being suspended between the two BLE connections.
public actor InMemoryWeatherFrameCache: WeatherFrameCache {
    private var entries: [WeatherFrameCacheKey: PrecipitationCrop] = [:]
    private var order: [WeatherFrameCacheKey] = []
    private let capacity: Int

    public init(capacity: Int = 64) {
        self.capacity = Swift.max(1, capacity)
    }

    public func crop(for key: WeatherFrameCacheKey) -> PrecipitationCrop? { entries[key] }

    public func store(_ crop: PrecipitationCrop, for key: WeatherFrameCacheKey) {
        if entries[key] == nil {
            order.append(key)
            while order.count > capacity {
                entries.removeValue(forKey: order.removeFirst())
            }
        }
        entries[key] = crop
    }
}

/// Disk cache with bounded retention.
///
/// Two rules earn their keep here. **Retention is bounded** by entry count, because frame keys are
/// timestamped and would otherwise accumulate one directory per publishing cycle forever. And
/// **corruption is a clean miss**: every entry carries a checksum of its own payload, and a file
/// that fails it (or fails to decode at all) is deleted and reported as absent. A cache is an
/// optimisation; the moment it can return something other than what was stored, it becomes a way to
/// draw rain that never fell.
public actor FileWeatherFrameCache: WeatherFrameCache {
    private let directory: URL
    private let capacity: Int
    private let fileManager = FileManager.default

    public init(directory: URL, capacity: Int = 256) {
        self.directory = directory
        self.capacity = Swift.max(1, capacity)
    }

    public func crop(for key: WeatherFrameCacheKey) -> PrecipitationCrop? {
        let url = url(for: key)
        guard let data = try? Data(contentsOf: url) else { return nil }
        guard let envelope = try? JSONDecoder().decode(Envelope.self, from: data),
              envelope.checksum == Envelope.checksum(of: envelope.cells),
              envelope.cells.count == envelope.width * envelope.height
        else {
            try? fileManager.removeItem(at: url)
            return nil
        }
        return envelope.crop
    }

    public func store(_ crop: PrecipitationCrop, for key: WeatherFrameCacheKey) {
        guard let data = try? JSONEncoder().encode(Envelope(crop: crop)) else { return }
        try? fileManager.createDirectory(at: directory, withIntermediateDirectories: true)
        try? data.write(to: url(for: key), options: .atomic)
        prune()
    }

    private func url(for key: WeatherFrameCacheKey) -> URL {
        // A key is a path-shaped object key; hashing keeps it one flat, filesystem-safe name.
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in key.identifier.utf8 {
            hash = (hash ^ UInt64(byte)) &* 0x0000_0100_0000_01B3
        }
        return directory.appendingPathComponent(String(hash, radix: 16) + ".obcwx")
    }

    private func prune() {
        guard let names = try? fileManager.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: [.contentModificationDateKey]),
            names.count > capacity
        else { return }
        let dated = names.map { url -> (URL, Date) in
            let date = (try? url.resourceValues(forKeys: [.contentModificationDateKey]))?
                .contentModificationDate ?? .distantPast
            return (url, date)
        }.sorted { $0.1 < $1.1 }
        for (url, _) in dated.prefix(names.count - capacity) {
            try? fileManager.removeItem(at: url)
        }
    }

    private struct Envelope: Codable {
        var validAt: Double
        var south: Int64
        var west: Int64
        var latitudeStride: UInt32
        var longitudeStride: UInt32
        var width: Int
        var height: Int
        var cellSizeMetres: UInt16
        var quality: UInt32
        var cells: Data
        var checksum: UInt32

        init(crop: PrecipitationCrop) {
            validAt = crop.validAt.timeIntervalSince1970
            south = crop.southMicrodegrees
            west = crop.westMicrodegrees
            latitudeStride = crop.latitudeStrideMicrodegrees
            longitudeStride = crop.longitudeStrideMicrodegrees
            width = crop.width
            height = crop.height
            cellSizeMetres = crop.cellSizeMetres
            quality = crop.quality.rawValue
            cells = Data(crop.cells)
            checksum = Envelope.checksum(of: cells)
        }

        var crop: PrecipitationCrop {
            PrecipitationCrop(
                validAt: Date(timeIntervalSince1970: validAt), southMicrodegrees: south,
                westMicrodegrees: west, latitudeStrideMicrodegrees: latitudeStride,
                longitudeStrideMicrodegrees: longitudeStride, width: width, height: height,
                cellSizeMetres: cellSizeMetres,
                quality: PrecipitationQuality(rawValue: quality), cells: [UInt8](cells))
        }

        /// A corruption check on cache bytes — deliberately *not* a wire contract. The OBC formats'
        /// CRC authority lives in `OBCWeatherWire`; duplicating it here would be a second authority
        /// for a job that only has to notice a truncated or flipped file.
        static func checksum(of bytes: Data) -> UInt32 {
            var hash: UInt32 = 2_166_136_261
            for byte in bytes { hash = (hash ^ UInt32(byte)) &* 16_777_619 }
            return hash
        }
    }
}
