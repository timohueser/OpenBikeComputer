import Foundation

/// §10's ObjectResult outcomes. Value `1` (`committedSupersededWeather`) is registered, reserved,
/// and never emitted in v3.0 — it exists as a decode-only row.
public enum ObjectOutcome: UInt16, Sendable, CaseIterable {
    case committed = 0
    case reservedSupersededWeather = 1
    case deleted = 2
    case metadataChanged = 3
    case updateInstallRequested = 4
    case rideImported = 5

    public var isReservedInV3: Bool { self == .reservedSupersededWeather }
}

/// §10, exactly 64 bytes. It carries no physical GenerationId — none is ever exposed.
public struct ObjectResult: Hashable, Sendable {
    public static let bodyBytes = 64

    public let operationId: OperationId
    public let storeId: StoreId
    public let objectKind: ObjectKind
    public let outcome: ObjectOutcome
    public let logicalObjectId: LogicalObjectId
    public let newRevision: Revision
    public let length: UInt64
    public let crc32: UInt32

    static func decode(_ reader: inout ByteReader) throws -> ObjectResult {
        let operationId = OperationId(unchecked: try reader.opaque16())
        let storeId = StoreId(unchecked: try reader.opaque16())
        let kindRaw = try reader.u16()
        guard let kind = ObjectKind(rawValue: kindRaw) else {
            throw WireFault.unknownEnum("ObjectResult: ObjectKind \(kindRaw)")
        }
        let outcomeRaw = try reader.u16()
        guard let outcome = ObjectOutcome(rawValue: outcomeRaw) else {
            throw WireFault.unknownEnum("ObjectResult: outcome \(outcomeRaw)")
        }
        return ObjectResult(
            operationId: operationId, storeId: storeId, objectKind: kind, outcome: outcome,
            logicalObjectId: LogicalObjectId(try reader.u64()),
            newRevision: Revision(try reader.u64()), length: try reader.u64(),
            crc32: try reader.u32())
    }

    func encode(into writer: inout ByteWriter) {
        writer.raw(operationId.bytes)
        writer.raw(storeId.bytes)
        writer.u16(objectKind.rawValue)
        writer.u16(outcome.rawValue)
        writer.u64(logicalObjectId.rawValue)
        writer.u64(newRevision.rawValue)
        writer.u64(length)
        writer.u32(crc32)
    }
}

/// §10, exactly 88 bytes. No LogicalObjectId and no GenerationId: a draft part is not a logical
/// object.
public struct DraftPartResult: Hashable, Sendable {
    public static let bodyBytes = 88

    public let childOperationId: OperationId
    public let storeId: StoreId
    public let parentOperationId: OperationId
    public let draftPartRef: DraftPartRef
    public let draftPartKind: DraftPartKind
    public let partKey: PartKey
    public let length: UInt64
    public let crc32: UInt32

    static func decode(_ reader: inout ByteReader) throws -> DraftPartResult {
        let child = OperationId(unchecked: try reader.opaque16())
        let store = StoreId(unchecked: try reader.opaque16())
        let parent = OperationId(unchecked: try reader.opaque16())
        let ref = DraftPartRef(unchecked: try reader.opaque16())
        let kindRaw = try reader.u16()
        guard let kind = DraftPartKind(rawValue: kindRaw) else {
            throw WireFault.unknownEnum("DraftPartResult: DraftPartKind \(kindRaw)")
        }
        try reader.reserved(2, "DraftPartResult offset 66")
        return DraftPartResult(
            childOperationId: child, storeId: store, parentOperationId: parent, draftPartRef: ref,
            draftPartKind: kind, partKey: PartKey(try reader.u64()), length: try reader.u64(),
            crc32: try reader.u32())
    }

    func encode(into writer: inout ByteWriter) {
        writer.raw(childOperationId.bytes)
        writer.raw(storeId.bytes)
        writer.raw(parentOperationId.bytes)
        writer.raw(draftPartRef.bytes)
        writer.u16(draftPartKind.rawValue)
        writer.zeros(2)
        writer.u64(partKey.rawValue)
        writer.u64(length)
        writer.u32(crc32)
    }
}

/// §10's AbortResult disposition.
public enum AbortDisposition: UInt8, Sendable, CaseIterable {
    case cancelled = 0
    case alreadyTerminal = 1
    case alreadyAbsent = 2
}

/// §10, exactly 56 bytes.
public struct AbortResult: Hashable, Sendable {
    public static let bodyBytes = 56

    public let abortCommandOperationId: OperationId
    public let storeId: StoreId
    public let targetOperationId: OperationId
    public let disposition: AbortDisposition

    static func decode(_ reader: inout ByteReader) throws -> AbortResult {
        let command = OperationId(unchecked: try reader.opaque16())
        let store = StoreId(unchecked: try reader.opaque16())
        let target = OperationId(unchecked: try reader.opaque16())
        let raw = try reader.u8()
        guard let disposition = AbortDisposition(rawValue: raw) else {
            throw WireFault.unknownEnum("AbortResult: disposition \(raw)")
        }
        try reader.reserved(7, "AbortResult offset 49")
        return AbortResult(
            abortCommandOperationId: command, storeId: store, targetOperationId: target,
            disposition: disposition)
    }

    func encode(into writer: inout ByteWriter) {
        writer.raw(abortCommandOperationId.bytes)
        writer.raw(storeId.bytes)
        writer.raw(targetOperationId.bytes)
        writer.u8(disposition.rawValue)
        writer.zeros(7)
    }
}

/// §10: `result_type u8`, three reserved zero bytes, then exactly one typed body.
///
/// The envelope carries no body length because it is always the final element of the payload that
/// contains it, so a decoder takes the remainder of the frame as its body and rejects any trailing
/// byte beyond the typed body's fixed size.
public enum ResultEnvelope: Hashable, Sendable {
    case object(ObjectResult)
    case draftPart(DraftPartResult)
    case abort(AbortResult)

    public static let headerBytes = 4

    public var resultType: UInt8 {
        switch self {
        case .object: return 1
        case .draftPart: return 2
        case .abort: return 3
        }
    }

    public var encodedLength: Int {
        switch self {
        case .object: return Self.headerBytes + ObjectResult.bodyBytes
        case .draftPart: return Self.headerBytes + DraftPartResult.bodyBytes
        case .abort: return Self.headerBytes + AbortResult.bodyBytes
        }
    }

    static func decode(_ reader: inout ByteReader) throws -> ResultEnvelope {
        let type = try reader.u8()
        try reader.reserved(3, "ResultEnvelope reserved")
        let envelope: ResultEnvelope
        switch type {
        case 1: envelope = .object(try ObjectResult.decode(&reader))
        case 2: envelope = .draftPart(try DraftPartResult.decode(&reader))
        case 3: envelope = .abort(try AbortResult.decode(&reader))
        default: throw WireFault.unknownEnum("ResultEnvelope: result_type \(type)")
        }
        try reader.requireExhausted("the typed body")
        return envelope
    }

    func encode(into writer: inout ByteWriter) {
        writer.u8(resultType)
        writer.zeros(3)
        switch self {
        case .object(let body): body.encode(into: &writer)
        case .draftPart(let body): body.encode(into: &writer)
        case .abort(let body): body.encode(into: &writer)
        }
    }
}
