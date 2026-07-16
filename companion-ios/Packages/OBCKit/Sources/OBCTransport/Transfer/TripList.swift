import Foundation
import OBCDomain

/// One `tripList` entry (type 10, spec §7.4) — the exact mirror of
/// ``RouteListEntry``: a 6-byte v2 header + fixed **76-byte** entries, so entry
/// `k` sits at `6 + 76k`. The device encodes (its trip catalog scan → the wire),
/// the app decodes (reconcile). Totals are summed over the trip's **resolvable**
/// stages; `stageCount` counts **every** stored stage (dangling refs included),
/// so it can exceed the number of stages the totals summed over.
///
/// Pinned byte-for-byte by `protocol-vectors/trip-list.bin` (`TripCodecTests`),
/// alongside the firmware `obc-ble` list codec.
public struct TripListEntry: Equatable, Sendable {
    public var objectID: UInt16
    /// Stored trip-object file size — sizes a reconcile download.
    public var byteLen: UInt32
    public var totalDistanceMeters: UInt32
    public var totalAscentMeters: UInt32
    public var stageCount: UInt16
    /// UTF-8, ≤ ``maxNameLength`` bytes; truncated at encode.
    public var name: String
    /// The stored trip object's whole-object CRC-32 (v2, spec §7.4) — the content
    /// fingerprint the app re-derives to detect an outdated trip (a stage reorder
    /// changes neither `byteLen` nor `name`). `0` = unknown, read the same by spec.
    public var crc32: UInt32

    /// The fixed entry size — mirrors ``RouteListEntry/encodedLength`` (both grew
    /// to 76 for the content CRC). Readers step by the header's `entryLen`.
    public static let encodedLength = 76
    /// The name-field cap (§7.4, matches the trip object's name field).
    public static let maxNameLength = 48

    public init(
        objectID: UInt16, byteLen: UInt32, totalDistanceMeters: UInt32,
        totalAscentMeters: UInt32, stageCount: UInt16, name: String, crc32: UInt32 = 0
    ) {
        self.objectID = objectID
        self.byteLen = byteLen
        self.totalDistanceMeters = totalDistanceMeters
        self.totalAscentMeters = totalAscentMeters
        self.stageCount = stageCount
        self.name = name
        self.crc32 = crc32
    }

    /// The reconcile-facing domain view (keyed by ``DeviceObjectID``) — the trip
    /// sibling of the `RouteListEntry → RouteCatalogEntry` map. Reconcile-only;
    /// never a list row.
    public var catalogEntry: TripCatalogEntry {
        TripCatalogEntry(
            id: DeviceObjectID(objectID),
            name: name,
            distanceMeters: Double(totalDistanceMeters),
            elevationGainMeters: Double(totalAscentMeters),
            stageCount: Int(stageCount),
            crc32: crc32
        )
    }

    public func encode() -> Data {
        var data = Data(count: Self.encodedLength)
        data.putLE(objectID, at: 0)
        // 2..4 reserved = 0
        data.putLE(byteLen, at: 4)
        data.putLE(totalDistanceMeters, at: 8)
        data.putLE(totalAscentMeters, at: 12)
        data.putLE(stageCount, at: 16)
        // 18..20 reserved = 0
        let nameBytes = Array(name.utf8.prefix(Self.maxNameLength))
        data[data.startIndex + 20] = UInt8(nameBytes.count)
        for (i, byte) in nameBytes.enumerated() { data[data.startIndex + 21 + i] = byte }
        // 21+n .. 69 name padding, 69..72 reserved[3] = 0
        data.putLE(crc32, at: 72)
        return data
    }

    /// Decode one entry from the first ``encodedLength`` bytes of an entry slot
    /// (a longer future entry's tail is ignored, per the header's `entryLen` rule).
    public init(decoding data: Data) throws {
        guard data.count >= Self.encodedLength else { throw DescriptorError.truncated }
        let b = data.startIndex
        let nameLen = Int(min(data[b + 20], UInt8(Self.maxNameLength)))
        self.init(
            objectID: data.getLE(at: 0),
            byteLen: data.getLE(at: 4),
            totalDistanceMeters: data.getLE(at: 8),
            totalAscentMeters: data.getLE(at: 12),
            stageCount: data.getLE(at: 16),
            name: String(decoding: data[(b + 21)..<(b + 21 + nameLen)], as: UTF8.self),
            crc32: data.getLE(at: 72)
        )
    }
}

/// The whole `tripList` object: the same 6-byte v2 header as ``RouteList`` +
/// packed 76-byte entries. Reconcile-only (like the route catalog) — never feeds
/// list rows.
public enum TripList {
    /// Decode a `tripList` object's entries — the shared v2 header walk, stepping
    /// by the header's announced `entryLen` (forward-compatible). `total` is
    /// decoded but not surfaced (trips, like routes, don't warn on truncation in
    /// v2; the per-entry `crc32` is the identity signal reconcile uses).
    public static func decode(_ data: Data) throws -> [TripListEntry] {
        guard data.count >= RouteList.headerLength else { throw DescriptorError.truncated }
        let b = data.startIndex
        guard data[b] == RouteList.version else { throw DescriptorError.unknownStatus(data[b]) }
        let entryLen = Int(data[b + 1])
        guard entryLen >= TripListEntry.encodedLength else { throw DescriptorError.unknownStatus(data[b + 1]) }
        let count: UInt16 = data.getLE(at: 2)
        var entries: [TripListEntry] = []
        entries.reserveCapacity(Int(count))
        for k in 0..<Int(count) {
            let start = b + RouteList.headerLength + k * entryLen
            guard start + entryLen <= data.endIndex else { throw DescriptorError.truncated }
            entries.append(try TripListEntry(decoding: data[start..<(start + entryLen)]))
        }
        return entries
    }

    /// Decode straight to the reconcile catalog — the "tripList decode →
    /// `TripCatalogEntry`" path the reconcile consumer (TR8) reads.
    public static func catalog(_ data: Data) throws -> [TripCatalogEntry] {
        try decode(data).map(\.catalogEntry)
    }

    /// Encode a `tripList` object (header + packed 76-byte entries) — the
    /// device's job, here for the vector round-trip pin. `total` = `count`.
    public static func encode(_ entries: [TripListEntry]) -> Data {
        var data = Data([RouteList.version, UInt8(TripListEntry.encodedLength)])
        data.appendLE(UInt16(entries.count))
        data.appendLE(UInt16(entries.count))  // total = count
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
