import Foundation

/// The 36 bytes DeleteObject and SetMetadata share (§9): `OperationId[16]`, ObjectKind `u16`, flags
/// `u16` whose expected-revision bit 0 is **mandatory**, LogicalObjectId `u64`, expected Revision.
public struct DirectMutationTarget: Hashable, Sendable {
    public static let prefixBytes = 36

    public let operationId: OperationId
    public let objectKind: ObjectKind
    public let logicalObjectId: LogicalObjectId
    public let expectedRevision: Revision

    static func decode(_ reader: inout ByteReader, subject: String) throws -> DirectMutationTarget {
        let operationId = OperationId(unchecked: try reader.opaque16())
        let kindRaw = try reader.u16()
        guard let kind = ObjectKind(rawValue: kindRaw) else {
            throw WireFault.unknownEnum("\(subject): ObjectKind \(kindRaw)")
        }
        let flags = try reader.u16()
        guard flags & ~UInt16(1) == 0 else {
            throw WireFault.reservedBits("\(subject): flags \(flags)")
        }
        guard flags & 1 == 1 else {
            throw WireFault.invalidCombination("\(subject): the expected-revision bit is mandatory")
        }
        return DirectMutationTarget(
            operationId: operationId, objectKind: kind,
            logicalObjectId: LogicalObjectId(try reader.u64()),
            expectedRevision: Revision(try reader.u64()))
    }

    func encode(into writer: inout ByteWriter) {
        writer.raw(operationId.bytes)
        writer.u16(objectKind.rawValue)
        writer.u16(1)
        writer.u64(logicalObjectId.rawValue)
        writer.u64(expectedRevision.rawValue)
    }
}

/// §9's 36-byte DeleteObject request.
public struct DeleteObjectRequest: Hashable, Sendable {
    public let target: DirectMutationTarget

    public static func decode(_ bytes: [UInt8]) throws -> DeleteObjectRequest {
        try requireExactPayload(bytes.count, DirectMutationTarget.prefixBytes, "DeleteObject")
        var reader = ByteReader(bytes, subject: "DeleteObject")
        return DeleteObjectRequest(
            target: try DirectMutationTarget.decode(&reader, subject: "DeleteObject"))
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        target.encode(into: &writer)
        return writer.bytes
    }
}

/// §9's SetMetadata request: the same 36 bytes followed by exactly one patch envelope.
public struct SetMetadataRequest: Hashable, Sendable {
    public let target: DirectMutationTarget
    public let patch: MetadataEnvelope

    public static func decode(_ bytes: [UInt8]) throws -> SetMetadataRequest {
        guard bytes.count >= DirectMutationTarget.prefixBytes + 8 else {
            throw WireFault.truncated("SetMetadata: \(bytes.count) bytes")
        }
        var reader = ByteReader(bytes, subject: "SetMetadata")
        let target = try DirectMutationTarget.decode(&reader, subject: "SetMetadata")
        let envelope = try MetadataEnvelope.decode(
            reader.rest(), maximumEncodedLength: SchemaClass.patch.envelopeCeiling)
        try envelope.validated(kind: target.objectKind, schemaClass: .patch, mutating: true)
        // §9: "A patch envelope is well-formed with zero fields, so an empty patch is not a codec
        // error; it is refused as a request."
        guard !envelope.fields.isEmpty else {
            throw WireFault.emptyMetadataPatch("SetMetadata: zero-field patch")
        }
        return SetMetadataRequest(target: target, patch: envelope)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        target.encode(into: &writer)
        writer.raw(try patch.encoded())
        return writer.bytes
    }
}

/// §9's 32-byte InstallUpdate and AcknowledgeRideImported requests. Both name their ObjectKind
/// implicitly — update package `7` and ride `3` respectively — which is why the canonical intent
/// suffix supplies it explicitly.
public struct KindedCommandRequest: Hashable, Sendable {
    public static let payloadBytes = 32

    public let operationId: OperationId
    public let logicalObjectId: LogicalObjectId
    public let expectedRevision: Revision
    /// The ObjectKind this opcode implies; never encoded in the request.
    public let impliedKind: ObjectKind

    public static func decode(_ bytes: [UInt8], impliedKind: ObjectKind, subject: String) throws
        -> KindedCommandRequest
    {
        try requireExactPayload(bytes.count, payloadBytes, subject)
        var reader = ByteReader(bytes, subject: subject)
        return KindedCommandRequest(
            operationId: OperationId(unchecked: try reader.opaque16()),
            logicalObjectId: LogicalObjectId(try reader.u64()),
            expectedRevision: Revision(try reader.u64()), impliedKind: impliedKind)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.raw(operationId.bytes)
        writer.u64(logicalObjectId.rawValue)
        writer.u64(expectedRevision.rawValue)
        return writer.bytes
    }
}
