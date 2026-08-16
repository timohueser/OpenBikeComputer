import Foundation

/// §7's 28-byte StartDownload request.
///
/// A download always resolves the *current committed head*: flag bit 0 and the eight bytes at
/// offset 12 carried a requested revision in an earlier draft of this contract and are burned, so a
/// nonzero encoding of either is `invalidDescriptor/reservedBits`.
public struct StartDownloadRequest: Hashable, Sendable {
    public struct Flags: OptionSet, Sendable, Hashable {
        public let rawValue: UInt16
        public init(rawValue: UInt16) { self.rawValue = rawValue }
        /// Burned; no v3.0 peer sets it.
        static let reservedRevision = Flags(rawValue: 1 << 0)
        public static let startOffset = Flags(rawValue: 1 << 1)
        static let defined: UInt16 = 0x0003
    }

    public let objectKind: ObjectKind
    public let startOffset: UInt64
    public let logicalObjectId: LogicalObjectId
    public let requestsStartOffset: Bool

    public static func decode(_ bytes: [UInt8]) throws -> StartDownloadRequest {
        try requireExactPayload(bytes.count, 28, "StartDownload")
        var reader = ByteReader(bytes, subject: "StartDownload")
        let kindRaw = try reader.u16()
        guard let kind = ObjectKind(rawValue: kindRaw) else {
            throw WireFault.unknownEnum("StartDownload: ObjectKind \(kindRaw)")
        }
        let flagsRaw = try reader.u16()
        guard flagsRaw & ~Flags.defined == 0 else {
            throw WireFault.reservedBits("StartDownload: flags \(flagsRaw)")
        }
        let flags = Flags(rawValue: flagsRaw)
        guard !flags.contains(.reservedRevision) else {
            throw WireFault.reservedBits("StartDownload: the burned revision flag is set")
        }
        let logicalId = LogicalObjectId(try reader.u64())
        try reader.reserved(8, "StartDownload: the burned revision field")
        let startOffset = try reader.u64()
        guard flags.contains(.startOffset) || startOffset == 0 else {
            throw WireFault.reservedBits("StartDownload: start offset without its flag")
        }
        return StartDownloadRequest(
            objectKind: kind, startOffset: startOffset, logicalObjectId: logicalId,
            requestsStartOffset: flags.contains(.startOffset))
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.u16(objectKind.rawValue)
        writer.u16(requestsStartOffset ? Flags.startOffset.rawValue : 0)
        writer.u64(logicalObjectId.rawValue)
        writer.zeros(8)
        writer.u64(startOffset)
        return writer.bytes
    }
}

/// §7's 60-byte DownloadAccepted. The accepted start offset always equals the offset the request
/// asked for — the device has no discretion to move it.
public struct DownloadAcceptance: Hashable, Sendable {
    public let storeId: StoreId
    public let sessionId: SessionId
    public let logicalObjectId: LogicalObjectId
    public let pinnedRevision: Revision
    public let totalLength: UInt64
    public let wholeSourceCRC32: UInt32
    public let acceptedStartOffset: UInt64
    public let maximumStreamPayload: UInt16

    public static func decode(_ bytes: [UInt8]) throws -> DownloadAcceptance {
        try requireExactPayload(bytes.count, 60, "DownloadAccepted")
        var reader = ByteReader(bytes, subject: "DownloadAccepted")
        let store = StoreId(unchecked: try reader.opaque16())
        guard let session = SessionId(try reader.u32()) else {
            throw WireFault.unknownEnum("DownloadAccepted: zero SessionId")
        }
        let logicalId = LogicalObjectId(try reader.u64())
        let revision = Revision(try reader.u64())
        let length = try reader.u64()
        let crc = try reader.u32()
        let start = try reader.u64()
        let maximumPayload = try reader.u16()
        try reader.reserved(2, "DownloadAccepted offset 58")
        return DownloadAcceptance(
            storeId: store, sessionId: session, logicalObjectId: logicalId,
            pinnedRevision: revision, totalLength: length, wholeSourceCRC32: crc,
            acceptedStartOffset: start, maximumStreamPayload: maximumPayload)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.raw(storeId.bytes)
        writer.u32(sessionId.rawValue)
        writer.u64(logicalObjectId.rawValue)
        writer.u64(pinnedRevision.rawValue)
        writer.u64(totalLength)
        writer.u32(wholeSourceCRC32)
        writer.u64(acceptedStartOffset)
        writer.u16(maximumStreamPayload)
        writer.zeros(2)
        return writer.bytes
    }
}

/// §7's 16-byte FinishDownload request. Length and CRC include a locally retained prefix when the
/// start offset was nonzero.
public struct FinishDownloadRequest: Hashable, Sendable {
    public let sessionId: SessionId
    public let receivedWholeSourceLength: UInt64
    public let wholeSourceCRC32: UInt32

    public static func decode(_ bytes: [UInt8]) throws -> FinishDownloadRequest {
        try requireExactPayload(bytes.count, 16, "FinishDownload")
        var reader = ByteReader(bytes, subject: "FinishDownload")
        guard let session = SessionId(try reader.u32()) else {
            throw WireFault.unknownEnum("FinishDownload: zero SessionId")
        }
        return FinishDownloadRequest(
            sessionId: session, receivedWholeSourceLength: try reader.u64(),
            wholeSourceCRC32: try reader.u32())
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.u32(sessionId.rawValue)
        writer.u64(receivedWholeSourceLength)
        writer.u32(wholeSourceCRC32)
        return writer.bytes
    }
}
