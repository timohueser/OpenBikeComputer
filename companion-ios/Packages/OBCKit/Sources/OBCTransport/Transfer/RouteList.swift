import Foundation

/// The `routeList` object (spec §7.4) — the catalog the device serves over the CoC (it outgrows the
/// 512-byte ATT cap fast). A 6-byte header + fixed-size entries, so entry `k` sits at
/// `6 + entryLen·k` — O(1) indexing, no string scanning. The entry grew to **84 bytes** with the
/// auto-expiry tail (epic #638): the 76-byte v2 core (through `crc32`) plus `expires_at u32 ·
/// retention u8 · reserved u8[3]`, appended **after** the content `crc32` (outside its coverage —
/// device-computed volatile state). Decode is `entryLen`-driven: a pre-expiry **76-byte** device
/// reads the core with both tail fields `nil`, an 84-byte device fills them, and a longer future
/// entry has its known 76-byte prefix decoded and its tail skipped.
///
/// The device encodes (its catalog scan → the wire); the app decodes (`listRoutes`). This mirrors
/// the firmware `obc-ble` `list` codec field-for-field and is pinned byte-for-byte by
/// `specs/vectors/route-list.bin` (`ProtocolVectorTests`), so neither side can drift from the
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
    /// The stored object's whole-object CRC-32 (v2, spec §7.4) — the content
    /// fingerprint for identity-verified route badges + adopt-by-content (V6 #770).
    /// `0` = unknown (a not-yet-filled side-load sidecar, or a genuine CRC of `0`,
    /// read the same by spec — no special-casing).
    public var crc32: UInt32
    /// The device's computed auto-delete instant, unix seconds (epic #638, the
    /// entry's `expires_at` tail at offset 76). `nil` when the entry carried no
    /// tail (a pre-expiry 76-byte device); a wire `0` (never / countdown not
    /// started) also decodes to `nil` — the app can't tell those apart and treats
    /// both as "no known expiry".
    public var expiresAt: UInt32?
    /// The device's stored retention level (epic #638, the entry's `retention`
    /// tail byte at offset 80). `nil` when the entry carried no tail; a byte is
    /// kept raw here (0…5) and sanitised at the domain boundary
    /// (``Retention/init(safeRawValue:)``).
    public var retention: UInt8?

    /// The v2 **core** entry size (spec §7.4, through `crc32`) — the smallest slot
    /// this codec can decode, and the `minEntryLen` the list walk enforces. A
    /// device advertising a shorter `entryLen` is rejected.
    public static let coreLength = 76
    /// The entry size *this codec writes* (the v2 core + the epic-#638 expiry
    /// tail). Readers step by the header's `entryLen`, not this constant, so a
    /// device serving the 76-byte core or a longer future entry both decode.
    public static let encodedLength = 84
    /// The name-field cap (§7.4, matches the OBCR route-name field).
    public static let maxNameLength = 48

    public init(
        objectID: UInt16, byteLen: UInt32, distanceMeters: UInt32, ascentMeters: UInt32,
        pointCount: UInt32, waypointCount: UInt16, name: String, crc32: UInt32 = 0,
        expiresAt: UInt32? = nil, retention: UInt8? = nil
    ) {
        self.objectID = objectID
        self.byteLen = byteLen
        self.distanceMeters = distanceMeters
        self.ascentMeters = ascentMeters
        self.pointCount = pointCount
        self.waypointCount = waypointCount
        self.name = name
        self.crc32 = crc32
        self.expiresAt = expiresAt
        self.retention = retention
    }

    public func encode() -> Data {
        var data = Data(count: Self.encodedLength)
        data.writeUInt16LE(objectID, at: 0)
        // 2..4 reserved = 0
        data.writeUInt32LE(byteLen, at: 4)
        data.writeUInt32LE(distanceMeters, at: 8)
        data.writeUInt32LE(ascentMeters, at: 12)
        data.writeUInt32LE(pointCount, at: 16)
        data.writeUInt16LE(waypointCount, at: 20)
        let nameBytes = Array(name.utf8.prefix(Self.maxNameLength))
        data[data.startIndex + 22] = UInt8(nameBytes.count)
        for (i, byte) in nameBytes.enumerated() { data[data.startIndex + 23 + i] = byte }
        // 23+n .. 71 zero padding
        data.writeUInt32LE(crc32, at: 72)
        // The auto-expiry tail (epic #638), after the content crc32: `nil` writes
        // `0` (the wire's "no known expiry" / "keep forever"), 76..80 zero padding.
        data.writeUInt32LE(expiresAt ?? 0, at: 76)
        data[data.startIndex + 80] = retention ?? 0
        return data
    }

    /// Decode one entry from an entry slot. Reads the 76-byte v2 core it always
    /// knows, then the epic-#638 expiry tail iff the slot carries it (`entryLen ≥
    /// 84`): `expires_at` (u32 LE @ 76; `0` → `nil`) + `retention` (u8 @ 80). A
    /// 76-byte slot leaves both `nil`; a longer future entry's extra tail is
    /// ignored, per the header's `entryLen` rule.
    public init(decoding data: Data) throws {
        guard data.count >= Self.coreLength else { throw DescriptorError.truncated }
        let b = data.startIndex
        let nameLen = Int(min(data[b + 22], UInt8(Self.maxNameLength)))
        let hasExpiryTail = data.count >= Self.encodedLength
        let rawExpiry: UInt32? = hasExpiryTail ? data.readUInt32LE(at: b + 76) : nil
        self.init(
            objectID: data.readUInt16LE(at: b),
            byteLen: data.readUInt32LE(at: b + 4),
            distanceMeters: data.readUInt32LE(at: b + 8),
            ascentMeters: data.readUInt32LE(at: b + 12),
            pointCount: data.readUInt32LE(at: b + 16),
            waypointCount: data.readUInt16LE(at: b + 20),
            name: String(decoding: data[(b + 23)..<(b + 23 + nameLen)], as: UTF8.self),
            crc32: data.readUInt32LE(at: b + 72),
            expiresAt: (rawExpiry ?? 0) == 0 ? nil : rawExpiry,
            retention: hasExpiryTail ? data[b + 80] : nil
        )
    }
}

/// The whole `routeList` object: its 6-byte v2 header + the packed entries.
public enum RouteList {
    /// `version u8 = 2 · entry_len u8 · count u16 · total u16` (spec §7.4).
    public static let headerLength = 6
    public static let version: UInt8 = 2

    /// Decode a `routeList` object's entries (the shared header + entry walk, see `decodeList`).
    /// The header's `total` is decoded but not surfaced here — routes don't warn on truncation
    /// in v2 (only rides do, via ``RideList``); `crc32` per entry is the identity signal V6 uses.
    public static func decode(_ data: Data) throws -> [RouteListEntry] {
        // `minEntryLen` is the v2 **core** (76), not what this codec writes (84):
        // a pre-expiry device serving 76-byte entries must still decode (both tail
        // fields `nil`), and the walk steps by the header's own `entryLen`.
        try decodeList(data, minEntryLen: RouteListEntry.coreLength) { try RouteListEntry(decoding: $0) }.entries
    }

    /// Encode a `routeList` object (header + packed 84-byte entries, the v2 core + the epic-#638
    /// expiry tail) — the device's job, here for the vector round-trip pin. `total` = `count` (a
    /// round-trip never models truncation).
    public static func encode(_ entries: [RouteListEntry]) -> Data {
        var data = Data([version, UInt8(RouteListEntry.encodedLength)])
        data.appendUInt16LE(UInt16(entries.count))
        data.appendUInt16LE(UInt16(entries.count))  // total = count
        for entry in entries { data.append(entry.encode()) }
        return data
    }
}

/// One `rideList` entry (spec §7.4) — from the stored ride-object header. Mirrors the firmware
/// `obc-ble` `RideListEntry` field-for-field; the device encodes (from A7), the app decodes
/// (`listRides`).
public struct RideListEntry: Equatable, Sendable {
    public var objectID: UInt16
    /// Stored file size — sizes the ride download.
    public var byteLen: UInt32
    /// Unix seconds.
    public var startTime: UInt32
    public var distanceMeters: UInt32
    public var movingTimeSeconds: UInt32
    public var averageSpeedCms: UInt16
    public var climbMeters: UInt16
    /// UTF-8, ≤ ``maxNameLength`` bytes; truncated at encode.
    public var name: String

    /// The fixed entry size (spec §7.4). Rides stay **72 bytes** in v2 — only the
    /// route entry grew (its content CRC); a ride's fixed fields already fill the
    /// slot. Its own constant so it can't drift with ``RouteListEntry/encodedLength``.
    public static let encodedLength = 72
    /// The name-field cap (§7.4 — one byte shorter than the route's: the fixed fields take one more).
    public static let maxNameLength = 47

    public init(
        objectID: UInt16, byteLen: UInt32, startTime: UInt32, distanceMeters: UInt32,
        movingTimeSeconds: UInt32, averageSpeedCms: UInt16, climbMeters: UInt16, name: String
    ) {
        self.objectID = objectID
        self.byteLen = byteLen
        self.startTime = startTime
        self.distanceMeters = distanceMeters
        self.movingTimeSeconds = movingTimeSeconds
        self.averageSpeedCms = averageSpeedCms
        self.climbMeters = climbMeters
        self.name = name
    }

    public func encode() -> Data {
        var data = Data(count: Self.encodedLength)
        data.writeUInt16LE(objectID, at: 0)
        // 2..4 reserved = 0
        data.writeUInt32LE(byteLen, at: 4)
        data.writeUInt32LE(startTime, at: 8)
        data.writeUInt32LE(distanceMeters, at: 12)
        data.writeUInt32LE(movingTimeSeconds, at: 16)
        data.writeUInt16LE(averageSpeedCms, at: 20)
        data.writeUInt16LE(climbMeters, at: 22)
        let nameBytes = Array(name.utf8.prefix(Self.maxNameLength))
        data[data.startIndex + 24] = UInt8(nameBytes.count)
        for (i, byte) in nameBytes.enumerated() { data[data.startIndex + 25 + i] = byte }
        return data
    }

    /// Decode one entry from the first 72 bytes of an entry slot (a longer future entry's tail is
    /// ignored, per the header's `entryLen` rule).
    public init(decoding data: Data) throws {
        guard data.count >= Self.encodedLength else { throw DescriptorError.truncated }
        let b = data.startIndex
        let nameLen = Int(min(data[b + 24], UInt8(Self.maxNameLength)))
        self.init(
            objectID: data.readUInt16LE(at: b),
            byteLen: data.readUInt32LE(at: b + 4),
            startTime: data.readUInt32LE(at: b + 8),
            distanceMeters: data.readUInt32LE(at: b + 12),
            movingTimeSeconds: data.readUInt32LE(at: b + 16),
            averageSpeedCms: data.readUInt16LE(at: b + 20),
            climbMeters: data.readUInt16LE(at: b + 22),
            name: String(decoding: data[(b + 25)..<(b + 25 + nameLen)], as: UTF8.self)
        )
    }
}

/// The whole `rideList` object — same 6-byte v2 header as `RouteList`, packed 72-byte entries.
public enum RideList {
    /// A decoded `rideList`: its entries plus the header's `total` (the full catalog size before the
    /// device's `MAX_RIDES` cap), so callers can surface truncation (`total > count`).
    public struct Decoded: Equatable, Sendable {
        public var entries: [RideListEntry]
        /// Full catalog size the header advertised — `entries.count` when nothing was capped.
        public var total: Int

        /// Rides the device holds beyond what the list carried (`total − count`), `0` when the
        /// whole catalog fit.
        public var hiddenCount: Int { max(0, total - entries.count) }
    }

    /// Decode a `rideList` object exactly as ``RouteList/decode(_:)`` does its entries, additionally
    /// surfacing the header's `total` for the truncation warning.
    public static func decode(_ data: Data) throws -> Decoded {
        let (entries, total) = try decodeList(data, minEntryLen: RideListEntry.encodedLength) {
            try RideListEntry(decoding: $0)
        }
        return Decoded(entries: entries, total: total)
    }

    /// Encode a `rideList` object — the device's job (A7); here for the round-trip pin. `total` =
    /// `count` (a round-trip never models truncation).
    public static func encode(_ entries: [RideListEntry]) -> Data {
        var data = Data([RouteList.version, UInt8(RideListEntry.encodedLength)])
        data.appendUInt16LE(UInt16(entries.count))
        data.appendUInt16LE(UInt16(entries.count))  // total = count
        for entry in entries { data.append(entry.encode()) }
        return data
    }
}

/// The shared list walk (spec §7.4): validate the 6-byte v2 header, then step `count` entries by the
/// header's announced `entryLen` — forward-compatible (a future longer entry has its known
/// `minEntryLen` prefix decoded and its tail skipped). Returns the decoded entries and the header's
/// `total` (the full catalog size the truncation flag compares against). `minEntryLen` is the
/// caller's known fixed-entry size (76 for routes, 72 for rides) — the smallest slot the codec can
/// decode; a header advertising a shorter `entryLen` is rejected.
private func decodeList<Entry>(
    _ data: Data, minEntryLen: Int, entry: (Data) throws -> Entry
) throws -> (entries: [Entry], total: Int) {
    guard data.count >= RouteList.headerLength else { throw DescriptorError.truncated }
    let b = data.startIndex
    guard data[b] == RouteList.version else { throw DescriptorError.unknownStatus(data[b]) }
    let entryLen = Int(data[b + 1])
    guard entryLen >= minEntryLen else { throw DescriptorError.unknownStatus(data[b + 1]) }
    let count = data.readUInt16LE(at: b + 2)
    let total = data.readUInt16LE(at: b + 4)
    var entries: [Entry] = []
    entries.reserveCapacity(Int(count))
    for k in 0..<Int(count) {
        let start = b + RouteList.headerLength + k * entryLen
        guard start + entryLen <= data.endIndex else { throw DescriptorError.truncated }
        entries.append(try entry(data[start..<(start + entryLen)]))
    }
    return (entries, Int(total))
}
