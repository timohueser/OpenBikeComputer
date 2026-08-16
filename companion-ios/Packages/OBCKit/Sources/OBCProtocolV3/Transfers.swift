import Foundation

/// §6.1's target mode. Zero is not a sentinel in either identity field; target mode alone
/// distinguishes the two encodings.
public enum TargetMode: UInt8, Sendable, CaseIterable {
    case create = 0
    case replace = 1
}

/// §6.1's resume *preference*. "A resume is never a reason to refuse an upload."
public enum ResumePreference: UInt8, Sendable, CaseIterable {
    case restartAtZero = 0
    case resumePermitted = 1
}

/// §6.1: all three resumable acceptances carry these flags identically — a `u16` at offset 2.
public struct ResumeFlags: OptionSet, Sendable, Hashable {
    public let rawValue: UInt16
    public init(rawValue: UInt16) { self.rawValue = rawValue }
    public static let resumedWork = ResumeFlags(rawValue: 1 << 0)
    public static let restartAtZero = ResumeFlags(rawValue: 1 << 1)
    static let defined: UInt16 = 0x0003

    static func decode(_ raw: UInt16, subject: String) throws -> ResumeFlags {
        guard raw & ~defined == 0 else {
            throw WireFault.reservedBits("\(subject): resume flags \(raw)")
        }
        let flags = ResumeFlags(rawValue: raw)
        // §6.1: "Restart-at-zero and resumed-work are never both set."
        guard !(flags.contains(.resumedWork) && flags.contains(.restartAtZero)) else {
            throw WireFault.invalidCombination("\(subject): both resume flags set")
        }
        return flags
    }
}

/// §6.1's 48-byte prefix plus exactly one metadata envelope. StartUpload is only a logical-object
/// Put.
public struct StartUploadRequest: Hashable, Sendable {
    public let operationId: OperationId
    public let objectKind: ObjectKind
    public let targetMode: TargetMode
    public let resume: ResumePreference
    public let logicalObjectId: LogicalObjectId
    public let expectedRevision: Revision
    public let declaredLength: UInt64
    public let expectedCRC32: UInt32
    public let metadata: MetadataEnvelope

    public static func decode(_ bytes: [UInt8]) throws -> StartUploadRequest {
        guard bytes.count >= WireLimits.startUploadPrefixBytes + 8 else {
            throw WireFault.truncated("StartUpload: \(bytes.count) bytes")
        }
        var reader = ByteReader(bytes, subject: "StartUpload")
        let operationId = OperationId(unchecked: try reader.opaque16())
        let kindRaw = try reader.u16()
        guard let kind = ObjectKind(rawValue: kindRaw) else {
            throw WireFault.unknownEnum("StartUpload: ObjectKind \(kindRaw)")
        }
        let modeRaw = try reader.u8()
        guard let mode = TargetMode(rawValue: modeRaw) else {
            throw WireFault.unknownEnum("StartUpload: target mode \(modeRaw)")
        }
        let resumeRaw = try reader.u8()
        guard let resume = ResumePreference(rawValue: resumeRaw) else {
            throw WireFault.unknownEnum("StartUpload: resume byte \(resumeRaw)")
        }
        let logicalId = LogicalObjectId(try reader.u64())
        let expectedRevision = Revision(try reader.u64())
        let declaredLength = try reader.u64()
        let crc = try reader.u32()
        // §6.1: "Create encodes logical ID and expected revision as zero… Other combinations are
        // invalidDescriptor." In replace mode both carry arbitrary opaque values, zero included.
        if mode == .create, logicalId.rawValue != 0 || expectedRevision.rawValue != 0 {
            throw WireFault.invalidCombination("StartUpload: create mode with a nonzero identity")
        }
        // The ceiling is this call site's fact, not the envelope's: a Put envelope sits in a
        // StartUpload descriptor, so §2.2's 128-byte Put/patch ceiling binds it whatever version
        // byte it happens to carry.
        let envelope = try MetadataEnvelope.decode(
            reader.rest(), maximumEncodedLength: SchemaClass.put.envelopeCeiling)
        try envelope.validated(kind: kind, schemaClass: .put, mutating: true)
        return StartUploadRequest(
            operationId: operationId, objectKind: kind, targetMode: mode, resume: resume,
            logicalObjectId: logicalId, expectedRevision: expectedRevision,
            declaredLength: declaredLength, expectedCRC32: crc, metadata: envelope)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.raw(operationId.bytes)
        writer.u16(objectKind.rawValue)
        writer.u8(targetMode.rawValue)
        writer.u8(resume.rawValue)
        writer.u64(logicalObjectId.rawValue)
        writer.u64(expectedRevision.rawValue)
        writer.u64(declaredLength)
        writer.u32(expectedCRC32)
        writer.raw(try metadata.encoded())
        return writer.bytes
    }
}

/// §6.1's UploadAccepted, frozen at exactly 64 bytes (vectors §2.1).
public struct UploadAcceptance: Hashable, Sendable {
    public static let acceptedBytes = 64

    public let targetMode: TargetMode
    public let flags: ResumeFlags
    public let operationId: OperationId
    public let sessionId: SessionId
    public let logicalObjectId: LogicalObjectId
    /// §6.1: a diagnostic snapshot of the repository at admission — **not** the next CAS token.
    public let repositoryRevisionAtAdmission: Revision
    public let durableNextOffset: UInt64
    public let checkpointGranule: UInt32
    public let maximumStreamPayload: UInt16
    public let finalizedPrefixCRC32: UInt32

    static func decodeAccepted(_ reader: inout ByteReader) throws -> UploadAcceptance {
        let modeRaw = try reader.u8()
        guard let mode = TargetMode(rawValue: modeRaw) else {
            throw WireFault.unknownEnum("UploadAccepted: target mode \(modeRaw)")
        }
        let flags = try ResumeFlags.decode(try reader.u16(), subject: "UploadAccepted")
        let operationId = OperationId(unchecked: try reader.opaque16())
        let sessionRaw = try reader.u32()
        guard let session = SessionId(sessionRaw) else {
            throw WireFault.unknownEnum("UploadAccepted: zero SessionId")
        }
        let logicalId = LogicalObjectId(try reader.u64())
        let revision = Revision(try reader.u64())
        let offset = try reader.u64()
        let granule = try reader.u32()
        let maximumPayload = try reader.u16()
        try reader.reserved(2, "UploadAccepted offset 54")
        let prefixCRC = try reader.u32()
        try reader.reserved(4, "UploadAccepted offset 60")
        try validatePrefix(flags: flags, offset: offset, crc: prefixCRC, subject: "UploadAccepted")
        return UploadAcceptance(
            targetMode: mode, flags: flags, operationId: operationId, sessionId: session,
            logicalObjectId: logicalId, repositoryRevisionAtAdmission: revision,
            durableNextOffset: offset, checkpointGranule: granule,
            maximumStreamPayload: maximumPayload, finalizedPrefixCRC32: prefixCRC)
    }

    func encodeAccepted(into writer: inout ByteWriter) {
        writer.u8(targetMode.rawValue)
        writer.u16(flags.rawValue)
        writer.raw(operationId.bytes)
        writer.u32(sessionId.rawValue)
        writer.u64(logicalObjectId.rawValue)
        writer.u64(repositoryRevisionAtAdmission.rawValue)
        writer.u64(durableNextOffset)
        writer.u32(checkpointGranule)
        writer.u16(maximumStreamPayload)
        writer.zeros(2)
        writer.u32(finalizedPrefixCRC32)
        writer.zeros(4)
    }
}

/// §6.1 + §6.2, shared by all three resumable acceptances: restart-at-zero forces both the reported
/// offset and the finalized prefix CRC to zero, and a zero offset is the only case in which a zero
/// CRC is not a computed value.
func validatePrefix(flags: ResumeFlags, offset: UInt64, crc: UInt32, subject: String) throws {
    if flags.contains(.restartAtZero), offset != 0 {
        throw WireFault.invalidCombination("\(subject): restart-at-zero with offset \(offset)")
    }
    if offset == 0, crc != 0 {
        throw WireFault.invalidCombination("\(subject): nonzero prefix CRC over an empty prefix")
    }
}

/// §6.2's 12-byte CheckpointUpload request.
public struct CheckpointUploadRequest: Hashable, Sendable {
    public let sessionId: SessionId
    public let receivedNextOffset: UInt64

    public static func decode(_ bytes: [UInt8]) throws -> CheckpointUploadRequest {
        try requireExactPayload(bytes.count, 12, "CheckpointUpload")
        var reader = ByteReader(bytes, subject: "CheckpointUpload")
        guard let session = SessionId(try reader.u32()) else {
            throw WireFault.unknownEnum("CheckpointUpload: zero SessionId")
        }
        return CheckpointUploadRequest(sessionId: session, receivedNextOffset: try reader.u64())
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.u32(sessionId.rawValue)
        writer.u64(receivedNextOffset)
        return writer.bytes
    }
}

/// §6.2's 20-byte checkpoint response.
public struct CheckpointUploadResponse: Hashable, Sendable {
    public let sessionId: SessionId
    public let durableNextOffset: UInt64
    public let finalizedPrefixCRC32: UInt32
    /// §6.2: starts at `1` for the first durable checkpoint of one work record, strictly increases,
    /// never wraps, and continues across a resume because it is scoped to the work record.
    public let checkpointSequence: UInt32

    public static func decode(_ bytes: [UInt8]) throws -> CheckpointUploadResponse {
        try requireExactPayload(bytes.count, 20, "CheckpointUpload response")
        var reader = ByteReader(bytes, subject: "CheckpointUpload response")
        guard let session = SessionId(try reader.u32()) else {
            throw WireFault.unknownEnum("CheckpointUpload response: zero SessionId")
        }
        let offset = try reader.u64()
        let crc = try reader.u32()
        let sequence = try reader.u32()
        guard sequence != 0 else {
            throw WireFault.invalidCombination("CheckpointUpload response: sequence 0")
        }
        return CheckpointUploadResponse(
            sessionId: session, durableNextOffset: offset, finalizedPrefixCRC32: crc,
            checkpointSequence: sequence)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.u32(sessionId.rawValue)
        writer.u64(durableNextOffset)
        writer.u32(finalizedPrefixCRC32)
        writer.u32(checkpointSequence)
        return writer.bytes
    }
}

/// §6.4's 8-byte AbortSession request.
public enum AbortReason: UInt8, Sendable, CaseIterable {
    case clientCancelled = 1
    case requestSuperseded = 2
    case userRequested = 3
}

public struct AbortSessionRequest: Hashable, Sendable {
    public let sessionId: SessionId
    public let reason: AbortReason

    public static func decode(_ bytes: [UInt8]) throws -> AbortSessionRequest {
        try requireExactPayload(bytes.count, 8, "AbortSession")
        var reader = ByteReader(bytes, subject: "AbortSession")
        guard let session = SessionId(try reader.u32()) else {
            throw WireFault.unknownEnum("AbortSession: zero SessionId")
        }
        let raw = try reader.u8()
        guard let reason = AbortReason(rawValue: raw) else {
            throw WireFault.unknownEnum("AbortSession: reason \(raw)")
        }
        try reader.reserved(3, "AbortSession offset 5")
        return AbortSessionRequest(sessionId: session, reason: reason)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.u32(sessionId.rawValue)
        writer.u8(reason.rawValue)
        writer.zeros(3)
        return writer.bytes
    }
}

/// §6.4's one-byte AbortSession response.
public enum AbortSessionOutcome: UInt8, Sendable, CaseIterable {
    case detached = 0
    case alreadyTerminal = 1
}

/// §6.4's 40-byte AbortOperation request.
public struct AbortOperationRequest: Hashable, Sendable {
    public let abortCommandOperationId: OperationId
    public let targetOperationId: OperationId
    public let reason: AbortReason

    public static func decode(_ bytes: [UInt8]) throws -> AbortOperationRequest {
        try requireExactPayload(bytes.count, 40, "AbortOperation")
        var reader = ByteReader(bytes, subject: "AbortOperation")
        let command = OperationId(unchecked: try reader.opaque16())
        let target = OperationId(unchecked: try reader.opaque16())
        let raw = try reader.u8()
        guard let reason = AbortReason(rawValue: raw) else {
            throw WireFault.unknownEnum("AbortOperation: reason \(raw)")
        }
        try reader.reserved(7, "AbortOperation offset 33")
        return AbortOperationRequest(
            abortCommandOperationId: command, targetOperationId: target, reason: reason)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.raw(abortCommandOperationId.bytes)
        writer.raw(targetOperationId.bytes)
        writer.u8(reason.rawValue)
        writer.zeros(7)
        return writer.bytes
    }
}

/// §6.5's 52-byte BeginDraft request.
public struct BeginDraftRequest: Hashable, Sendable {
    public let parentOperationId: OperationId
    public let finalObjectKind: ObjectKind
    public let targetMode: TargetMode
    public let logicalObjectId: LogicalObjectId
    public let expectedRevision: Revision
    public let declaredManifestLength: UInt64
    public let declaredManifestCRC32: UInt32
    public let expectedPartCount: UInt16

    public static func decode(_ bytes: [UInt8]) throws -> BeginDraftRequest {
        try requireExactPayload(bytes.count, 52, "BeginDraft")
        var reader = ByteReader(bytes, subject: "BeginDraft")
        let parent = OperationId(unchecked: try reader.opaque16())
        let kindRaw = try reader.u16()
        guard let kind = ObjectKind(rawValue: kindRaw) else {
            throw WireFault.unknownEnum("BeginDraft: ObjectKind \(kindRaw)")
        }
        let modeRaw = try reader.u8()
        guard let mode = TargetMode(rawValue: modeRaw) else {
            throw WireFault.unknownEnum("BeginDraft: target mode \(modeRaw)")
        }
        try reader.reserved(1, "BeginDraft offset 19")
        let logicalId = LogicalObjectId(try reader.u64())
        let expectedRevision = Revision(try reader.u64())
        let length = try reader.u64()
        let crc = try reader.u32()
        let partCount = try reader.u16()
        try reader.reserved(2, "BeginDraft offset 50")

        // §6.5: only a kind whose operation flags permit draft finalization is valid; v3.0 uses
        // volume manifest.
        guard kind == .volumeManifest else {
            throw WireFault.unsupportedLogicalKind("BeginDraft: \(kind.name) cannot be finalized from a draft")
        }
        if mode == .create, logicalId.rawValue != 0 || expectedRevision.rawValue != 0 {
            throw WireFault.invalidCombination("BeginDraft: create mode with a nonzero identity")
        }
        // §6.5 + registries §2.1: nonzero and no greater than the advertised maximum, whose ceiling
        // is the 32-part budget of §5.1.
        guard partCount >= 1, partCount <= 32 else {
            throw WireFault.invalidCombination("BeginDraft: part count \(partCount)")
        }
        return BeginDraftRequest(
            parentOperationId: parent, finalObjectKind: kind, targetMode: mode,
            logicalObjectId: logicalId, expectedRevision: expectedRevision,
            declaredManifestLength: length, declaredManifestCRC32: crc,
            expectedPartCount: partCount)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.raw(parentOperationId.bytes)
        writer.u16(finalObjectKind.rawValue)
        writer.u8(targetMode.rawValue)
        writer.u8(0)
        writer.u64(logicalObjectId.rawValue)
        writer.u64(expectedRevision.rawValue)
        writer.u64(declaredManifestLength)
        writer.u32(declaredManifestCRC32)
        writer.u16(expectedPartCount)
        writer.zeros(2)
        return writer.bytes
    }
}

/// §6.5's BeginDraft disposition-0 body: a four-byte disposition/reserved prefix plus 28 bytes.
public struct BeginDraftAcceptance: Hashable, Sendable {
    public enum State: UInt8, Sendable, CaseIterable { case open = 0 }

    public let parentOperationId: OperationId
    public let draftRevision: DraftRevision
    public let expectedParts: UInt16
    public let state: State

    static func decode(_ reader: inout ByteReader) throws -> BeginDraftAcceptance {
        let parent = OperationId(unchecked: try reader.opaque16())
        let revision = DraftRevision(try reader.u64())
        let parts = try reader.u16()
        let stateRaw = try reader.u8()
        guard let state = State(rawValue: stateRaw) else {
            throw WireFault.unknownEnum("BeginDraftAccepted: state \(stateRaw)")
        }
        try reader.reserved(1, "BeginDraftAccepted offset 27")
        return BeginDraftAcceptance(
            parentOperationId: parent, draftRevision: revision, expectedParts: parts, state: state)
    }

    func encode(into writer: inout ByteWriter) {
        writer.raw(parentOperationId.bytes)
        writer.u64(draftRevision.rawValue)
        writer.u16(expectedParts)
        writer.u8(state.rawValue)
        writer.zeros(1)
    }
}

/// §6.5's 64-byte StartDraftPart request.
public struct StartDraftPartRequest: Hashable, Sendable {
    public let childOperationId: OperationId
    public let parentOperationId: OperationId
    public let draftPartKind: DraftPartKind
    public let partKey: PartKey
    public let declaredLength: UInt64
    public let expectedCRC32: UInt32
    public let resume: ResumePreference

    public static func decode(_ bytes: [UInt8]) throws -> StartDraftPartRequest {
        try requireExactPayload(bytes.count, 64, "StartDraftPart")
        var reader = ByteReader(bytes, subject: "StartDraftPart")
        let child = OperationId(unchecked: try reader.opaque16())
        let parent = OperationId(unchecked: try reader.opaque16())
        let kindRaw = try reader.u16()
        guard let kind = DraftPartKind(rawValue: kindRaw) else {
            throw WireFault.unknownEnum("StartDraftPart: DraftPartKind \(kindRaw)")
        }
        try reader.reserved(2, "StartDraftPart offset 34")
        let key = PartKey(try reader.u64())
        let length = try reader.u64()
        let crc = try reader.u32()
        let resumeRaw = try reader.u8()
        guard let resume = ResumePreference(rawValue: resumeRaw) else {
            throw WireFault.unknownEnum("StartDraftPart: resume byte \(resumeRaw)")
        }
        try reader.reserved(7, "StartDraftPart offset 57")
        // §6.5: "The child OperationId must be distinct from the parent."
        guard child != parent else {
            throw WireFault.invalidCombination("StartDraftPart: child OperationId equals the parent")
        }
        return StartDraftPartRequest(
            childOperationId: child, parentOperationId: parent, draftPartKind: kind, partKey: key,
            declaredLength: length, expectedCRC32: crc, resume: resume)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.raw(childOperationId.bytes)
        writer.raw(parentOperationId.bytes)
        writer.u16(draftPartKind.rawValue)
        writer.zeros(2)
        writer.u64(partKey.rawValue)
        writer.u64(declaredLength)
        writer.u32(expectedCRC32)
        writer.u8(resume.rawValue)
        writer.zeros(7)
        return writer.bytes
    }
}

/// §6.5's DraftPartAccepted, frozen at exactly 72 bytes (vectors §2.1). It carries no DraftPartRef:
/// that reference does not exist until sealing is durable.
public struct DraftPartAcceptance: Hashable, Sendable {
    public static let acceptedBytes = 72

    public let flags: ResumeFlags
    public let childOperationId: OperationId
    public let parentOperationId: OperationId
    public let sessionId: SessionId
    public let draftPartKind: DraftPartKind
    public let partKey: PartKey
    public let durableNextOffset: UInt64
    public let checkpointGranule: UInt32
    public let maximumStreamPayload: UInt16
    public let finalizedPrefixCRC32: UInt32

    static func decodeAccepted(_ reader: inout ByteReader) throws -> DraftPartAcceptance {
        try reader.reserved(1, "DraftPartAccepted offset 1")
        let flags = try ResumeFlags.decode(try reader.u16(), subject: "DraftPartAccepted")
        let child = OperationId(unchecked: try reader.opaque16())
        let parent = OperationId(unchecked: try reader.opaque16())
        guard let session = SessionId(try reader.u32()) else {
            throw WireFault.unknownEnum("DraftPartAccepted: zero SessionId")
        }
        let kindRaw = try reader.u16()
        guard let kind = DraftPartKind(rawValue: kindRaw) else {
            throw WireFault.unknownEnum("DraftPartAccepted: DraftPartKind \(kindRaw)")
        }
        try reader.reserved(2, "DraftPartAccepted offset 42")
        let key = PartKey(try reader.u64())
        let offset = try reader.u64()
        let granule = try reader.u32()
        let maximumPayload = try reader.u16()
        try reader.reserved(2, "DraftPartAccepted offset 66")
        let crc = try reader.u32()
        try validatePrefix(flags: flags, offset: offset, crc: crc, subject: "DraftPartAccepted")
        return DraftPartAcceptance(
            flags: flags, childOperationId: child, parentOperationId: parent, sessionId: session,
            draftPartKind: kind, partKey: key, durableNextOffset: offset,
            checkpointGranule: granule, maximumStreamPayload: maximumPayload,
            finalizedPrefixCRC32: crc)
    }

    func encodeAccepted(into writer: inout ByteWriter) {
        writer.zeros(1)
        writer.u16(flags.rawValue)
        writer.raw(childOperationId.bytes)
        writer.raw(parentOperationId.bytes)
        writer.u32(sessionId.rawValue)
        writer.u16(draftPartKind.rawValue)
        writer.zeros(2)
        writer.u64(partKey.rawValue)
        writer.u64(durableNextOffset)
        writer.u32(checkpointGranule)
        writer.u16(maximumStreamPayload)
        writer.zeros(2)
        writer.u32(finalizedPrefixCRC32)
    }
}

/// §6.5's FinalizeDraft acceptance, frozen at exactly 64 bytes (vectors §2.1).
public struct FinalizeDraftAcceptance: Hashable, Sendable {
    public static let acceptedBytes = 64

    public let flags: ResumeFlags
    public let parentOperationId: OperationId
    public let sessionId: SessionId
    public let logicalObjectId: LogicalObjectId
    public let repositoryRevisionAtAdmission: Revision
    public let durableManifestOffset: UInt64
    public let checkpointGranule: UInt32
    public let maximumStreamPayload: UInt16
    public let finalizedPrefixCRC32: UInt32

    static func decodeAccepted(_ reader: inout ByteReader) throws -> FinalizeDraftAcceptance {
        try reader.reserved(1, "FinalizeDraft acceptance offset 1")
        let flags = try ResumeFlags.decode(try reader.u16(), subject: "FinalizeDraft acceptance")
        let parent = OperationId(unchecked: try reader.opaque16())
        guard let session = SessionId(try reader.u32()) else {
            throw WireFault.unknownEnum("FinalizeDraft acceptance: zero SessionId")
        }
        let logicalId = LogicalObjectId(try reader.u64())
        let revision = Revision(try reader.u64())
        let offset = try reader.u64()
        let granule = try reader.u32()
        let maximumPayload = try reader.u16()
        try reader.reserved(2, "FinalizeDraft acceptance offset 54")
        let crc = try reader.u32()
        try reader.reserved(4, "FinalizeDraft acceptance offset 60")
        try validatePrefix(
            flags: flags, offset: offset, crc: crc, subject: "FinalizeDraft acceptance")
        return FinalizeDraftAcceptance(
            flags: flags, parentOperationId: parent, sessionId: session,
            logicalObjectId: logicalId, repositoryRevisionAtAdmission: revision,
            durableManifestOffset: offset, checkpointGranule: granule,
            maximumStreamPayload: maximumPayload, finalizedPrefixCRC32: crc)
    }

    func encodeAccepted(into writer: inout ByteWriter) {
        writer.zeros(1)
        writer.u16(flags.rawValue)
        writer.raw(parentOperationId.bytes)
        writer.u32(sessionId.rawValue)
        writer.u64(logicalObjectId.rawValue)
        writer.u64(repositoryRevisionAtAdmission.rawValue)
        writer.u64(durableManifestOffset)
        writer.u32(checkpointGranule)
        writer.u16(maximumStreamPayload)
        writer.zeros(2)
        writer.u32(finalizedPrefixCRC32)
        writer.zeros(4)
    }
}

/// The two dispositions every `Start*` acceptance shares: `0` accepted, `1` already terminal.
public enum AcceptanceDisposition: UInt8, Sendable, CaseIterable {
    case accepted = 0
    case alreadyTerminal = 1
}
