import Foundation
import OBCDomain

/// The trip-object codec: the phone-side encoder for a whole-trip upload and the reader for a
/// reconcile download. A trip object references one route per day by its device object id and never
/// carries route bytes.
///
/// The layout is little-endian (`obc-ble-interface-spec.md` §7.7): a fixed 64-byte header, then one
/// 16-byte record per day in ride order. The header holds the version, the day count, a
/// length-prefixed, zero-padded name of at most 48 bytes, the start date and the trip key.
///
/// Compaction is the caller's job, not the codec's: the app hands `encode` the already-resolved
/// device ids of the days that still exist, so an upload never carries a dangling ref. `decode` is
/// byte-faithful and returns exactly the days the object holds, dangling refs included, because the
/// device tolerates them on read and never rewrites a stored trip.
///
/// Pinned byte for byte against a shared vector the firmware side pins too, so neither can drift
/// from the spec without a test going red.
public enum TripObjectCodec {
    /// The trip object format version this codec writes and reads.
    public static let version: UInt8 = 3
    /// The fixed header size; the day records follow it.
    static let headerLength = 64
    static let dayLength = 16
    /// Name-field cap, matching the device and the wire config name.
    static let nameCap = 48
    private static let nameLengthOffset = 4
    private static let nameOffset = 5
    private static let startDateOffset = 54
    private static let keyOffset = 56

    /// One day: its route's device object id and where that route runs on the trip's main line.
    public struct Day: Equatable, Sendable {
        public var routeID: DeviceObjectID
        /// Metres along the day route where it joins the main line.
        public var joinMeters: UInt32
        /// Metres along the day route where it leaves the main line. A value at or past the
        /// route's end means the day ends on the line.
        public var leaveMeters: UInt32

        public init(routeID: DeviceObjectID, joinMeters: UInt32, leaveMeters: UInt32) {
            self.routeID = routeID
            self.joinMeters = joinMeters
            self.leaveMeters = leaveMeters
        }

        /// A day that starts and ends on the main line.
        public static func whole(_ routeID: DeviceObjectID) -> Day {
            Day(routeID: routeID, joinMeters: 0, leaveMeters: .max)
        }
    }

    /// The contents of a trip object.
    public struct Trip: Equatable, Sendable {
        /// The phone's stable trip key, nonzero. A re-upload of the same trip keeps it.
        public var key: UInt64
        public var name: String
        /// Days since 1970-01-01; 0 = no start date.
        public var startDate: UInt16
        /// The days in ride order. Byte-faithful on decode, so a stored dangling ref is present.
        public var days: [Day]

        public init(key: UInt64, name: String, startDate: UInt16, days: [Day]) {
            self.key = key
            self.name = name
            self.startDate = startDate
            self.days = days
        }
    }

    // MARK: Encode

    /// Encode a trip object. The name is truncated to ``nameCap`` UTF-8 bytes on a character
    /// boundary, and the days to `UInt16.max`.
    public static func encode(_ trip: Trip) -> Data {
        let days = trip.days.prefix(Int(UInt16.max))
        var data = Data(count: startDateOffset)
        data[data.startIndex] = version
        // Byte 1 is reserved and stays zero.
        data.writeUInt16LE(UInt16(days.count), at: 2)
        let nameBytes = Data(trip.name.truncatedToUTF8Bytes(nameCap).utf8)
        data[data.startIndex + nameLengthOffset] = UInt8(nameBytes.count)
        for (i, byte) in nameBytes.enumerated() { data[data.startIndex + nameOffset + i] = byte }
        // The name padding and the reserved byte 53 are already zero.
        data.appendUInt16LE(trip.startDate)
        data.appendUInt64LE(trip.key)
        for day in days {
            data.appendUInt64LE(day.routeID.raw)
            data.appendUInt32LE(day.joinMeters)
            data.appendUInt32LE(day.leaveMeters)
        }
        return data
    }

    /// The CRC-32 of the trip object an upload of `trip` would send: the trip-level
    /// ``OnDeviceState`` fingerprint.
    public static func payloadCRC(_ trip: Trip) -> UInt32 {
        CRC32.checksum(encode(trip))
    }

    // MARK: Decode

    /// Decode a trip object. Every field is reached by an explicit offset and bounds-checked, so
    /// malformed device bytes throw ``DeviceError/readFailed`` and never trap. The length must be
    /// exactly `64 + 16·day_count`, which also rejects a torn write, and the key must be nonzero.
    public static func decode(_ data: Data) throws -> Trip {
        guard data.count >= headerLength else { throw DeviceError.readFailed }
        let b = data.startIndex
        guard data[b] == version else { throw DeviceError.readFailed }
        let dayCount = Int(data.readUInt16LE(at: b + 2))
        guard data.count == headerLength + dayCount * dayLength else { throw DeviceError.readFailed }
        let nameLen = Int(min(data[b + nameLengthOffset], UInt8(nameCap)))
        let name = String(decoding: data[(b + nameOffset)..<(b + nameOffset + nameLen)], as: UTF8.self)
        let days = (0..<dayCount).map { k in
            let at = b + headerLength + k * dayLength
            return Day(
                routeID: DeviceObjectID(data.readUInt64LE(at: at)),
                joinMeters: data.readUInt32LE(at: at + 8),
                leaveMeters: data.readUInt32LE(at: at + 12))
        }
        let key = data.readUInt64LE(at: b + keyOffset)
        // The device reads key 0 as "no trip".
        guard key != 0 else { throw DeviceError.readFailed }
        return Trip(
            key: key, name: name,
            startDate: data.readUInt16LE(at: b + startDateOffset), days: days)
    }
}
