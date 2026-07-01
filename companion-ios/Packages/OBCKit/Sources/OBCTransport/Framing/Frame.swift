import Foundation

/// The kind of object a frame carries — the `type` field of the frame header
/// (`OBCProtocol.md` → *CoC framing*). `firmware` is **reserved** for a future OTA
/// type; this epic ships no codec for it.
public enum ObjectType: UInt8, Equatable, Sendable, CaseIterable {
    case route = 1
    case ride = 2
    case configBlob = 3
    case diagnostics = 4
    case firmware = 5  // reserved (OTA) — no codec in this epic
}

/// Provisional wire constants for the frame header.
///
/// > **Pinned by firmware `S0`, mirrored here.** `OBCProtocol.md` names the field
/// > widths / endianness as firmware-owned. These are the self-consistent values
/// > `B1` builds + tests against; when `S0` freezes the numbers, change them **in
/// > this one enum** and the codec + every test follow. The *semantics* (chunking,
/// > offset-resume, CRC-before-commit) are contract-level and won't move.
///
/// Header layout — little-endian, **19 bytes**, CRC over `[type … chunkLen] ++ payload`:
/// ```
///   offset  field      width
///   0       type       u8
///   1       objectID   u16
///   3       totalLen   u32
///   7       offset     u32
///   11      chunkLen   u32
///   15      crc32      u32   ← covers bytes 0..<15 and the payload
/// ```
public enum FrameFormat {
    /// Bytes of header preceding the CRC (`type` … `chunkLen`).
    public static let prefixSize = 15
    /// Full header size including the trailing CRC.
    public static let headerSize = 19
    /// Default payload bytes per frame. A framing choice, not MTU-bound (the CoC is
    /// a byte stream that the link layer segments); small enough for fine-grained
    /// resume, large enough to amortize header overhead.
    public static let defaultChunkSize = 4096
}

/// A parsed frame header. `crc32` is the value carried on the wire; validate it
/// against the payload with ``FrameCodec/verify(_:payload:)``.
public struct FrameHeader: Equatable, Sendable {
    public var type: ObjectType
    public var objectID: UInt16
    public var totalLen: UInt32
    public var offset: UInt32
    public var chunkLen: UInt32
    public var crc32: UInt32

    public init(type: ObjectType, objectID: UInt16, totalLen: UInt32, offset: UInt32, chunkLen: UInt32, crc32: UInt32) {
        self.type = type
        self.objectID = objectID
        self.totalLen = totalLen
        self.offset = offset
        self.chunkLen = chunkLen
        self.crc32 = crc32
    }
}

/// Why a frame failed to parse/validate. `crcMismatch` maps to
/// `DeviceError.crcMismatch` at the transport surface.
public enum FramingError: Error, Equatable, Sendable {
    /// Fewer bytes than a header (or a truncated payload).
    case truncated
    /// `type` byte isn't a known `ObjectType`.
    case unknownType(UInt8)
    /// Recomputed CRC ≠ the frame's `crc32` — the object is rejected, never committed.
    case crcMismatch
}

/// Encode/decode of a single frame — pure byte math, **no CoreBluetooth**, so the
/// whole thing round-trips under `swift test` with no hardware.
public enum FrameCodec {
    /// Serialize one frame: header (with a freshly computed CRC) + payload.
    public static func encode(type: ObjectType, objectID: UInt16, totalLen: UInt32, offset: UInt32, payload: Data) -> Data {
        var prefix = Data(capacity: FrameFormat.prefixSize)
        prefix.append(type.rawValue)
        prefix.appendLE(objectID)
        prefix.appendLE(totalLen)
        prefix.appendLE(offset)
        prefix.appendLE(UInt32(payload.count))

        var crcInput = prefix
        crcInput.append(payload)
        let crc = CRC32.checksum(crcInput)

        var frame = prefix
        frame.appendLE(crc)
        frame.append(payload)
        return frame
    }

    /// Parse the fixed-size header (exactly `headerSize` bytes). Does **not** check
    /// the CRC — that needs the payload; call ``verify(_:payload:)`` after reading
    /// `chunkLen` payload bytes.
    public static func parseHeader(_ bytes: Data) throws -> FrameHeader {
        guard bytes.count >= FrameFormat.headerSize else { throw FramingError.truncated }
        let b = bytes.startIndex
        guard let type = ObjectType(rawValue: bytes[b]) else { throw FramingError.unknownType(bytes[b]) }
        return FrameHeader(
            type: type,
            objectID: bytes.readLE(at: b + 1),
            totalLen: bytes.readLE(at: b + 3),
            offset: bytes.readLE(at: b + 7),
            chunkLen: bytes.readLE(at: b + 11),
            crc32: bytes.readLE(at: b + 15)
        )
    }

    /// Recompute the CRC over `header` + `payload` and reject on mismatch.
    public static func verify(_ header: FrameHeader, payload: Data) throws {
        var crcInput = Data(capacity: FrameFormat.prefixSize + payload.count)
        crcInput.append(header.type.rawValue)
        crcInput.appendLE(header.objectID)
        crcInput.appendLE(header.totalLen)
        crcInput.appendLE(header.offset)
        crcInput.appendLE(header.chunkLen)
        crcInput.append(payload)
        guard CRC32.checksum(crcInput) == header.crc32 else { throw FramingError.crcMismatch }
    }
}

// MARK: - Little-endian (de)serialization

extension Data {
    fileprivate mutating func appendLE(_ value: UInt16) {
        append(UInt8(value & 0xFF))
        append(UInt8((value >> 8) & 0xFF))
    }

    fileprivate mutating func appendLE(_ value: UInt32) {
        append(UInt8(value & 0xFF))
        append(UInt8((value >> 8) & 0xFF))
        append(UInt8((value >> 16) & 0xFF))
        append(UInt8((value >> 24) & 0xFF))
    }

    fileprivate func readLE(at index: Index) -> UInt16 {
        UInt16(self[index]) | (UInt16(self[index + 1]) << 8)
    }

    fileprivate func readLE(at index: Index) -> UInt32 {
        UInt32(self[index]) | (UInt32(self[index + 1]) << 8)
            | (UInt32(self[index + 2]) << 16) | (UInt32(self[index + 3]) << 24)
    }
}
