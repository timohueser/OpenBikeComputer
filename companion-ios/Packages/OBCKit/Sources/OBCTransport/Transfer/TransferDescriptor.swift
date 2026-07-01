import Foundation

/// The kind of object a bulk transfer carries. `firmware` is **reserved** for a
/// future OTA type — no codec in this epic.
public enum ObjectType: UInt8, Equatable, Sendable, CaseIterable {
    case route = 1
    case ride = 2
    case configBlob = 3
    case diagnostics = 4
    case firmware = 5  // reserved (OTA)
}

/// **Control-plane** descriptor that opens a bulk transfer (over the GATT
/// `TransferControl` characteristic), before any payload byte crosses the CoC. It
/// carries *all* the metadata — so the CoC itself is just **raw payload bytes**,
/// with no per-chunk headers for the MCU to parse or buffer.
///
/// Fixed **15 bytes**, little-endian — a couple of loads for the MCU:
/// ```
///   type          u8    ObjectType
///   objectID      u16
///   totalLen      u32   full object size
///   crc32         u32   whole-object CRC-32/IEEE, checked at commit
///   resumeOffset  u32   byte offset to start streaming from (0 = fresh)
/// ```
///
/// > **Provisional pending firmware `S0`** (owns field widths/endianness). Centralized
/// > here + in `CRC32`/`GATT` for a one-spot repin.
public struct TransferStart: Equatable, Sendable {
    public var type: ObjectType
    public var objectID: UInt16
    public var totalLen: UInt32
    public var crc32: UInt32
    public var resumeOffset: UInt32

    public static let encodedLength = 15

    public init(type: ObjectType, objectID: UInt16, totalLen: UInt32, crc32: UInt32, resumeOffset: UInt32 = 0) {
        self.type = type
        self.objectID = objectID
        self.totalLen = totalLen
        self.crc32 = crc32
        self.resumeOffset = resumeOffset
    }

    public func encode() -> Data {
        var data = Data(capacity: Self.encodedLength)
        data.append(type.rawValue)
        data.appendLE(objectID)
        data.appendLE(totalLen)
        data.appendLE(crc32)
        data.appendLE(resumeOffset)
        return data
    }

    public init(decoding data: Data) throws {
        guard data.count >= Self.encodedLength else { throw DescriptorError.truncated }
        let b = data.startIndex
        guard let type = ObjectType(rawValue: data[b]) else { throw DescriptorError.unknownType(data[b]) }
        self.init(
            type: type,
            objectID: data.readLE(at: b + 1),
            totalLen: data.readLE(at: b + 3),
            crc32: data.readLE(at: b + 7),
            resumeOffset: data.readLE(at: b + 11)
        )
    }
}

/// **Control-plane** result the device notifies (over `Status`) at the end of — or
/// to resume — a transfer. `committedOffset` is the durable byte count: the resume
/// anchor a dropped transfer restarts from.
///
/// Fixed **7 bytes**, little-endian: `objectID u16 · status u8 · committedOffset u32`.
public struct TransferResult: Equatable, Sendable {
    public enum Status: UInt8, Equatable, Sendable {
        case committed = 0   // stored + CRC verified
        case crcMismatch = 1 // rejected, not committed
        case aborted = 2     // canceled by either side
        case error = 3
    }

    public var objectID: UInt16
    public var status: Status
    public var committedOffset: UInt32

    public static let encodedLength = 7

    public init(objectID: UInt16, status: Status, committedOffset: UInt32) {
        self.objectID = objectID
        self.status = status
        self.committedOffset = committedOffset
    }

    public func encode() -> Data {
        var data = Data(capacity: Self.encodedLength)
        data.appendLE(objectID)
        data.append(status.rawValue)
        data.appendLE(committedOffset)
        return data
    }

    public init(decoding data: Data) throws {
        guard data.count >= Self.encodedLength else { throw DescriptorError.truncated }
        let b = data.startIndex
        guard let status = Status(rawValue: data[b + 2]) else { throw DescriptorError.unknownStatus(data[b + 2]) }
        self.init(objectID: data.readLE(at: b), status: status, committedOffset: data.readLE(at: b + 3))
    }
}

/// Why a control-plane descriptor failed to decode.
public enum DescriptorError: Error, Equatable, Sendable {
    case truncated
    case unknownType(UInt8)
    case unknownStatus(UInt8)
}

// MARK: - Little-endian (de)serialization

extension Data {
    fileprivate mutating func appendLE(_ value: UInt16) {
        append(UInt8(value & 0xFF)); append(UInt8((value >> 8) & 0xFF))
    }

    fileprivate mutating func appendLE(_ value: UInt32) {
        append(UInt8(value & 0xFF)); append(UInt8((value >> 8) & 0xFF))
        append(UInt8((value >> 16) & 0xFF)); append(UInt8((value >> 24) & 0xFF))
    }

    fileprivate func readLE(at index: Index) -> UInt16 {
        UInt16(self[index]) | (UInt16(self[index + 1]) << 8)
    }

    fileprivate func readLE(at index: Index) -> UInt32 {
        UInt32(self[index]) | (UInt32(self[index + 1]) << 8)
            | (UInt32(self[index + 2]) << 16) | (UInt32(self[index + 3]) << 24)
    }
}
