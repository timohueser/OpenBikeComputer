import Foundation

/// The `routeList` object (spec §7.4) — the catalog the device serves over the CoC (it outgrows the
/// 512-byte ATT cap fast). A 4-byte header + fixed **72-byte** entries, so entry `k` sits at
/// `4 + 72k` — O(1) indexing, no string scanning.
///
/// The device encodes (its catalog scan → the wire); the app decodes (`listRoutes`). This mirrors
/// the firmware `obc-ble` `list` codec field-for-field and is pinned byte-for-byte by
/// `protocol-vectors/route-list.bin` (`ProtocolVectorTests`), so neither side can drift from the
/// spec without a test going red.
public struct RouteListEntry: Equatable, Sendable {
    public var objectID: UInt16
    /// Stored file size — sizes an upload/detail transfer.
    public var byteLen: UInt32
    public var distanceMeters: UInt32
    public var ascentMeters: UInt32
    public var pointCount: UInt32
    public var waypointCount: UInt16
    /// UTF-8, ≤ ``maxNameLength`` bytes (the OBCR route-name cap); truncated at encode.
    public var name: String

    /// The fixed entry size (spec §7.4). Readers step by the header's `entryLen`, not this constant,
    /// so a future longer entry stays forward-compatible; this is what *this* codec writes.
    public static let encodedLength = 72
    /// The name-field cap (§7.4, matches the OBCR route-name field).
    public static let maxNameLength = 48

    public init(
        objectID: UInt16, byteLen: UInt32, distanceMeters: UInt32, ascentMeters: UInt32,
        pointCount: UInt32, waypointCount: UInt16, name: String
    ) {
        self.objectID = objectID
        self.byteLen = byteLen
        self.distanceMeters = distanceMeters
        self.ascentMeters = ascentMeters
        self.pointCount = pointCount
        self.waypointCount = waypointCount
        self.name = name
    }

    public func encode() -> Data {
        var data = Data(count: Self.encodedLength)
        data.putLE(objectID, at: 0)
        // 2..4 reserved = 0
        data.putLE(byteLen, at: 4)
        data.putLE(distanceMeters, at: 8)
        data.putLE(ascentMeters, at: 12)
        data.putLE(pointCount, at: 16)
        data.putLE(waypointCount, at: 20)
        let nameBytes = Array(name.utf8.prefix(Self.maxNameLength))
        data[data.startIndex + 22] = UInt8(nameBytes.count)
        for (i, byte) in nameBytes.enumerated() { data[data.startIndex + 23 + i] = byte }
        // 23+n .. 71 zero padding; 71 reserved = 0
        return data
    }

    /// Decode one entry from the first ``encodedLength`` bytes of an entry slot (a longer future
    /// entry's tail is ignored, per the header's `entryLen` rule).
    public init(decoding data: Data) throws {
        guard data.count >= Self.encodedLength else { throw DescriptorError.truncated }
        let b = data.startIndex
        let nameLen = Int(min(data[b + 22], UInt8(Self.maxNameLength)))
        self.init(
            objectID: data.getLE(at: 0),
            byteLen: data.getLE(at: 4),
            distanceMeters: data.getLE(at: 8),
            ascentMeters: data.getLE(at: 12),
            pointCount: data.getLE(at: 16),
            waypointCount: data.getLE(at: 20),
            name: String(decoding: data[(b + 23)..<(b + 23 + nameLen)], as: UTF8.self)
        )
    }
}

/// The whole `routeList` object: its 4-byte header + the packed entries.
public enum RouteList {
    /// `version u8 = 1 · entry_len u8 = 72 · count u16` (spec §7.4).
    public static let headerLength = 4
    public static let version: UInt8 = 1

    /// Decode a `routeList` object: validate the header, then step `count` entries by the header's
    /// announced `entryLen` — forward-compatible, so a future longer entry has its known 72-byte
    /// prefix decoded and its tail skipped.
    public static func decode(_ data: Data) throws -> [RouteListEntry] {
        guard data.count >= headerLength else { throw DescriptorError.truncated }
        let b = data.startIndex
        guard data[b] == version else { throw DescriptorError.unknownStatus(data[b]) }
        let entryLen = Int(data[b + 1])
        guard entryLen >= RouteListEntry.encodedLength else { throw DescriptorError.unknownStatus(data[b + 1]) }
        let count: UInt16 = data.getLE(at: 2)
        var entries: [RouteListEntry] = []
        entries.reserveCapacity(Int(count))
        for k in 0..<Int(count) {
            let start = b + headerLength + k * entryLen
            guard start + entryLen <= data.endIndex else { throw DescriptorError.truncated }
            entries.append(try RouteListEntry(decoding: data[start..<(start + entryLen)]))
        }
        return entries
    }

    /// Encode a `routeList` object (header + packed 72-byte entries) — the device's job, here for the
    /// vector round-trip pin.
    public static func encode(_ entries: [RouteListEntry]) -> Data {
        var data = Data([version, UInt8(RouteListEntry.encodedLength)])
        data.appendLE(UInt16(entries.count))
        for entry in entries { data.append(entry.encode()) }
        return data
    }
}

// MARK: - Little-endian (de)serialization

extension Data {
    fileprivate mutating func appendLE(_ value: UInt16) {
        append(UInt8(value & 0xFF)); append(UInt8((value >> 8) & 0xFF))
    }

    fileprivate mutating func putLE(_ value: UInt16, at offset: Int) {
        let i = startIndex + offset
        self[i] = UInt8(value & 0xFF); self[i + 1] = UInt8((value >> 8) & 0xFF)
    }

    fileprivate mutating func putLE(_ value: UInt32, at offset: Int) {
        let i = startIndex + offset
        self[i] = UInt8(value & 0xFF); self[i + 1] = UInt8((value >> 8) & 0xFF)
        self[i + 2] = UInt8((value >> 16) & 0xFF); self[i + 3] = UInt8((value >> 24) & 0xFF)
    }

    fileprivate func getLE(at offset: Int) -> UInt16 {
        let i = startIndex + offset
        return UInt16(self[i]) | (UInt16(self[i + 1]) << 8)
    }

    fileprivate func getLE(at offset: Int) -> UInt32 {
        let i = startIndex + offset
        return UInt32(self[i]) | (UInt32(self[i + 1]) << 8)
            | (UInt32(self[i + 2]) << 16) | (UInt32(self[i + 3]) << 24)
    }
}
