import Foundation
import OBCDomain

/// The **trip object** codec (type 9, spec §7.7) — the phone-side encoder for a
/// whole-trip upload and the reader for a reconcile download. A trip object
/// *references* routes by their device object id; it never carries route bytes
/// (a route stays a byte-identical OBCR file, membership edits never touch it).
///
/// Layout (little-endian): a fixed **56-byte header** followed by
/// `stage_count × u64` route object ids in ride order:
/// ```
/// version    u8  = 2
/// reserved   u8
/// stage_count u16
/// name_len   u8   ≤ 48
/// name       char[48]  (zero-padded)
/// reserved   u8[3]
/// stages     u64[stage_count]   flat-store object ids, ride order
/// ```
/// **Compaction is the caller's job, not the codec's.** The app owns validation
/// and hands `encode` the already-resolved, ride-ordered device ids for the
/// stages that still exist — so an upload built from resolvable stages never
/// carries a dangling ref. `decode` is byte-faithful: it returns exactly the ids
/// the object holds (dangling refs included — the device tolerates them on read
/// and never rewrites a stored trip), so decode∘encode round-trips byte-exactly.
///
/// Pinned byte-for-byte against `specs/vectors/trip-v2.bin`
/// (`TripCodecTests`), which the firmware side pins too, so neither can drift
/// from the spec without a test going red.
public enum TripObjectCodec {
    /// The trip object format version this codec writes (spec §7.7). Public so the
    /// mock's device-side trip store and the vector tests can stamp it.
    public static let version: UInt8 = 2
    /// The fixed header size (spec §7.7); stage ids follow it.
    static let headerLength = 56
    static let stageIDLength = 8
    /// Name-field cap (matches the device `NAME_CAP` / the wire `Config` name).
    static let nameCap = 48
    /// Header offset of `name_len`.
    private static let nameLengthOffset = 4
    /// Header offset of the zero-padded name field.
    private static let nameOffset = 5

    // MARK: Encode

    /// Encode a trip object from its display name and the resolved device object
    /// ids of its stages, **in ride order** — the caller resolves
    /// `TripRecord.stageIDs` to device ids (passing `trip.name` explicitly) and
    /// drops any unresolvable stage (the compaction the spec's
    /// stages-first-trip-last upload relies on). Deliberately **not** a
    /// `TripRecord` overload: the record's `stageIDs` are library ids the codec
    /// can't resolve, so a record-taking signature would silently ignore half
    /// its input. `stage_count` is `deviceStageIDs.count`; the name is truncated
    /// to ``nameCap`` UTF-8 bytes on a character boundary.
    public static func encode(name: String, deviceStageIDs: [DeviceObjectID]) -> Data {
        let stages = deviceStageIDs.prefix(Int(UInt16.max))
        var data = Data(count: headerLength)
        data[data.startIndex] = version
        // [1] reserved = 0
        data.writeUInt16LE(UInt16(stages.count), at: 2)
        let nameBytes = truncatedUTF8(name, maxBytes: nameCap)
        data[data.startIndex + nameLengthOffset] = UInt8(nameBytes.count)
        for (i, byte) in nameBytes.enumerated() { data[data.startIndex + nameOffset + i] = byte }
        // name padding + reserved[3] already zero
        for id in stages { data.appendUInt64LE(id.raw) }
        return data
    }

    /// The CRC-32 of the trip object an upload of this trip would send — the
    /// trip-level ``OnDeviceState`` fingerprint (the sibling of
    /// `RouteObjectCodec.payloadCRC`). The one canonical "payload for a trip"
    /// definition: the trip's name + its resolved stage ids in ride order.
    public static func payloadCRC(name: String, deviceStageIDs: [DeviceObjectID]) -> UInt32 {
        CRC32.checksum(encode(name: name, deviceStageIDs: deviceStageIDs))
    }

    // MARK: Decode

    /// The parsed contents of a trip object — the name and the stage device ids
    /// in ride order (dangling refs included, exactly as stored).
    public struct Decoded: Equatable, Sendable {
        public var version: UInt8
        public var name: String
        /// Stage route object ids in ride order — byte-faithful, so a stored
        /// dangling ref (a member route deleted individually) is present here.
        public var stageObjectIDs: [DeviceObjectID]

        public init(version: UInt8, name: String, stageObjectIDs: [DeviceObjectID]) {
            self.version = version
            self.name = name
            self.stageObjectIDs = stageObjectIDs
        }
    }

    /// Decode a trip object. Every field is reached by explicit offset and
    /// bounds-checked — malformed device bytes throw ``DeviceError/readFailed``,
    /// never trap.
    public static func decode(_ data: Data) throws -> Decoded {
        guard data.count >= headerLength else { throw DeviceError.readFailed }
        let b = data.startIndex
        let version = data[b]
        guard version == Self.version else { throw DeviceError.readFailed }
        let stageCount = Int(data.readUInt16LE(at: b + 2))
        let nameLen = Int(min(data[b + nameLengthOffset], UInt8(nameCap)))
        let name = String(decoding: data[(b + nameOffset)..<(b + nameOffset + nameLen)], as: UTF8.self)

        let stagesStart = b + headerLength
        guard stagesStart + stageCount * stageIDLength <= data.endIndex else { throw DeviceError.readFailed }
        var stages: [DeviceObjectID] = []
        stages.reserveCapacity(stageCount)
        for k in 0..<stageCount {
            let offset = b + headerLength + k * stageIDLength
            stages.append(DeviceObjectID(data.readUInt64LE(at: offset)))
        }
        return Decoded(version: version, name: name, stageObjectIDs: stages)
    }

    /// UTF-8 bytes of `string`, truncated to at most `maxBytes` on a character
    /// boundary (never splitting a multi-byte scalar) — the same rule the route
    /// and config name fields use.
    private static func truncatedUTF8(_ string: String, maxBytes: Int) -> Data {
        var bytes = Data()
        for character in string {
            let encoded = Array(String(character).utf8)
            if bytes.count + encoded.count > maxBytes { break }
            bytes.append(contentsOf: encoded)
        }
        return bytes
    }
}
