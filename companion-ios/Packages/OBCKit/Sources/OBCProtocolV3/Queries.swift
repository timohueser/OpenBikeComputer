import Foundation

// MARK: - QueryOperation (§8.1)

/// §8.1's phase enum, projected from the storage contract's phase names.
public enum OperationPhase: UInt8, Sendable, CaseIterable {
    case prepared = 0
    case streaming = 1
    case sealed = 2
    case validating = 3
    case publishing = 4
    case externalHandoff = 5
    case draftOpen = 6
    case aborting = 7
}

public enum ProgressSubjectNamespace: UInt8, Sendable, CaseIterable {
    case none = 0
    case logicalObjectKind = 1
    case draftPartKind = 2
}

public struct ProgressFlags: OptionSet, Sendable, Hashable {
    public let rawValue: UInt8
    public init(rawValue: UInt8) { self.rawValue = rawValue }
    public static let resumable = ProgressFlags(rawValue: 1 << 0)
    /// §8.1: attachment is advisory and grants no ownership.
    public static let sessionCurrentlyAttached = ProgressFlags(rawValue: 1 << 1)
    public static let logicalIdPresent = ProgressFlags(rawValue: 1 << 2)
    static let defined: UInt8 = 0x07
}

/// §8.1's 24-byte progress body.
public struct OperationProgress: Hashable, Sendable {
    public static let bodyBytes = 24

    public let subjectNamespace: ProgressSubjectNamespace
    public let phase: OperationPhase
    public let flags: ProgressFlags
    public let subjectKind: UInt16
    public let logicalObjectId: LogicalObjectId
    public let durableOffset: UInt64

    static func decode(_ reader: inout ByteReader) throws -> OperationProgress {
        let namespaceRaw = try reader.u8()
        guard let namespace = ProgressSubjectNamespace(rawValue: namespaceRaw) else {
            throw WireFault.unknownEnum("QueryOperation progress: namespace \(namespaceRaw)")
        }
        let phaseRaw = try reader.u8()
        guard let phase = OperationPhase(rawValue: phaseRaw) else {
            throw WireFault.unknownEnum("QueryOperation progress: phase \(phaseRaw)")
        }
        let flagsRaw = try reader.u8()
        guard flagsRaw & ~ProgressFlags.defined == 0 else {
            throw WireFault.reservedBits("QueryOperation progress: flags bits 3…7")
        }
        let flags = ProgressFlags(rawValue: flagsRaw)
        try reader.reserved(1, "QueryOperation progress offset 3")
        let kind = try reader.u16()
        try reader.reserved(2, "QueryOperation progress offset 6")
        let logicalId = LogicalObjectId(try reader.u64())
        let offset = try reader.u64()

        // §8.1's matrix, in the part a decoder can check from the body alone.
        switch namespace {
        case .none:
            guard kind == 0, flags.isEmpty, logicalId.rawValue == 0, offset == 0 else {
                throw WireFault.invalidCombination(
                    "QueryOperation progress: namespace none with a nonzero field")
            }
        case .logicalObjectKind:
            guard ObjectKind(rawValue: kind) != nil else {
                throw WireFault.unknownEnum("QueryOperation progress: ObjectKind \(kind)")
            }
        case .draftPartKind:
            guard DraftPartKind(rawValue: kind) != nil else {
                throw WireFault.unknownEnum("QueryOperation progress: DraftPartKind \(kind)")
            }
            // §8.1: a draft-part claim has ID-present clear and ID zero.
            guard !flags.contains(.logicalIdPresent), logicalId.rawValue == 0 else {
                throw WireFault.invalidCombination(
                    "QueryOperation progress: a draft part reports a LogicalObjectId")
            }
        }
        // §8.1: "An ID field with ID-present clear is zero. With the bit set, zero remains a valid
        // opaque LogicalObjectId."
        if !flags.contains(.logicalIdPresent), logicalId.rawValue != 0 {
            throw WireFault.invalidCombination(
                "QueryOperation progress: LogicalObjectId set with ID-present clear")
        }
        return OperationProgress(
            subjectNamespace: namespace, phase: phase, flags: flags, subjectKind: kind,
            logicalObjectId: logicalId, durableOffset: offset)
    }

    func encode(into writer: inout ByteWriter) {
        writer.u8(subjectNamespace.rawValue)
        writer.u8(phase.rawValue)
        writer.u8(flags.rawValue)
        writer.zeros(1)
        writer.u16(subjectKind)
        writer.zeros(2)
        writer.u64(logicalObjectId.rawValue)
        writer.u64(durableOffset)
    }
}

/// §8.1's four states. `Unknown` means only that the ID is neither active nor retained — it cannot
/// distinguish never-claimed from evicted.
public enum QueryOperationState: Hashable, Sendable {
    case unknown
    case inProgress(OperationProgress)
    case committed(ResultEnvelope)
    /// §8.1: a *successful* query answer carrying the retained text-free ErrorBody.
    case aborted(ErrorBody)

    var stateByte: UInt8 {
        switch self {
        case .unknown: return 0
        case .inProgress: return 1
        case .committed: return 2
        case .aborted: return 3
        }
    }

    public static func decode(_ bytes: [UInt8]) throws -> QueryOperationState {
        guard bytes.count >= 4 else {
            throw WireFault.truncated("QueryOperation response: \(bytes.count) bytes")
        }
        var reader = ByteReader(bytes, subject: "QueryOperation response")
        let state = try reader.u8()
        try reader.reserved(3, "QueryOperation response offset 1")
        switch state {
        case 0:
            try reader.requireExhausted("state Unknown")
            return .unknown
        case 1:
            let progress = try OperationProgress.decode(&reader)
            try reader.requireExhausted("the progress body")
            return .inProgress(progress)
        case 2:
            return .committed(try ResultEnvelope.decode(&reader))
        case 3:
            let body = try ErrorBody.decode(reader.rest())
            guard body.text.isEmpty else {
                throw WireFault.invalidCombination(
                    "QueryOperation response: Aborted carries diagnostic text")
            }
            return .aborted(body)
        default:
            throw WireFault.unknownEnum("QueryOperation response: state \(state)")
        }
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.u8(stateByte)
        writer.zeros(3)
        switch self {
        case .unknown: break
        case .inProgress(let progress): progress.encode(into: &writer)
        case .committed(let envelope): envelope.encode(into: &writer)
        case .aborted(let body): writer.raw(try body.encoded())
        }
        return writer.bytes
    }
}

// MARK: - QueryCatalog (§8.2)

/// §8.2's cursor: repository Revision, next entry index, ObjectKind, and CRC-32 over the current
/// StoreId followed by those first 12 bytes. Opaque to application code despite its normative codec.
public struct CatalogCursor: Hashable, Sendable {
    public static let byteCount = 16
    public let bytes: [UInt8]

    /// Internal, like the opaque identities: every accessor below indexes fixed offsets, so a
    /// cursor is 16 bytes by construction rather than by convention.
    init(unchecked bytes: [UInt8]) { self.bytes = bytes }

    /// Fails unless exactly 16 bytes are supplied.
    public init?(bytes: [UInt8]) {
        guard bytes.count == Self.byteCount else { return nil }
        self.bytes = bytes
    }

    public var isZero: Bool { bytes.allSatisfy { $0 == 0 } }

    public var revision: Revision {
        var reader = ByteReader(bytes, subject: "catalog cursor")
        return Revision((try? reader.u64()) ?? 0)
    }
    public var nextEntryIndex: UInt16 {
        UInt16(bytes[8]) | UInt16(bytes[9]) << 8
    }
    public var objectKindCode: UInt16 {
        UInt16(bytes[10]) | UInt16(bytes[11]) << 8
    }
    public var checksum: UInt32 {
        var v: UInt32 = 0
        for i in (12..<16).reversed() { v = v << 8 | UInt32(bytes[i]) }
        return v
    }

    /// The CRC a device computes for this cursor under `storeId`.
    public func expectedChecksum(storeId: StoreId) -> UInt32 {
        CRC32IEEE.checksum(storeId.bytes + bytes[0..<12])
    }
}

public struct QueryCatalogRequest: Hashable, Sendable {
    public struct Flags: OptionSet, Sendable, Hashable {
        public let rawValue: UInt16
        public init(rawValue: UInt16) { self.rawValue = rawValue }
        public static let expectedRevision = Flags(rawValue: 1 << 0)
        public static let cursor = Flags(rawValue: 1 << 1)
        static let defined: UInt16 = 0x0003
    }

    public let objectKind: ObjectKind
    public let flags: Flags
    public let expectedRevision: Revision
    public let cursor: CatalogCursor

    public static func decode(_ bytes: [UInt8]) throws -> QueryCatalogRequest {
        try requireExactPayload(bytes.count, 28, "QueryCatalog")
        var reader = ByteReader(bytes, subject: "QueryCatalog")
        let kindRaw = try reader.u16()
        guard let kind = ObjectKind(rawValue: kindRaw) else {
            throw WireFault.unknownEnum("QueryCatalog: ObjectKind \(kindRaw)")
        }
        let flagsRaw = try reader.u16()
        guard flagsRaw & ~Flags.defined == 0 else {
            throw WireFault.reservedBits("QueryCatalog: flags \(flagsRaw)")
        }
        let flags = Flags(rawValue: flagsRaw)
        let revision = Revision(try reader.u64())
        let cursor = CatalogCursor(unchecked: try reader.opaque16())

        if !flags.contains(.expectedRevision) {
            guard revision.rawValue == 0 else {
                throw WireFault.reservedBits("QueryCatalog: expected revision without its flag")
            }
        }
        if flags.contains(.cursor) {
            // §8.2: "Cursor requires both bits and an expected revision equal to the cursor
            // revision."
            guard flags.contains(.expectedRevision) else {
                throw WireFault.invalidCombination("QueryCatalog: cursor flag alone")
            }
            guard cursor.revision == revision else {
                throw WireFault.invalidCombination(
                    "QueryCatalog: cursor revision \(cursor.revision) != expected \(revision)")
            }
            guard cursor.objectKindCode == kind.rawValue else {
                throw WireFault.invalidCombination("QueryCatalog: cursor names another ObjectKind")
            }
        } else {
            guard cursor.isZero else {
                throw WireFault.reservedBits("QueryCatalog: cursor bytes without the cursor flag")
            }
        }
        return QueryCatalogRequest(
            objectKind: kind, flags: flags, expectedRevision: revision, cursor: cursor)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.u16(objectKind.rawValue)
        writer.u16(flags.rawValue)
        writer.u64(expectedRevision.rawValue)
        writer.raw(cursor.bytes)
        return writer.bytes
    }
}

/// One catalog entry: §8.2's 36-byte prefix plus exactly that many metadata bytes.
public struct CatalogEntry: Hashable, Sendable {
    public static let prefixBytes = 36

    public let logicalObjectId: LogicalObjectId
    public let revision: Revision
    public let length: UInt64
    public let crc32: UInt32
    public let metadata: MetadataEnvelope
}

/// §8.2's catalog page: a 44-byte prefix plus whole entries.
public struct CatalogPage: Hashable, Sendable {
    public let storeId: StoreId
    public let objectKind: ObjectKind
    public let repositoryRevision: Revision
    public let nextCursor: CatalogCursor
    public let entries: [CatalogEntry]

    /// `more` lives in the control header, so it is passed in: the next cursor is zero unless it is
    /// set.
    public static func decode(_ bytes: [UInt8], more: Bool) throws -> CatalogPage {
        guard bytes.count >= WireLimits.catalogPagePrefixBytes else {
            throw WireFault.truncated("QueryCatalog response: \(bytes.count) bytes")
        }
        var reader = ByteReader(bytes, subject: "QueryCatalog response")
        let store = StoreId(unchecked: try reader.opaque16())
        let kindRaw = try reader.u16()
        guard let kind = ObjectKind(rawValue: kindRaw) else {
            throw WireFault.unknownEnum("QueryCatalog response: ObjectKind \(kindRaw)")
        }
        let entryCount = Int(try reader.u16())
        let revision = Revision(try reader.u64())
        let cursor = CatalogCursor(unchecked: try reader.opaque16())
        guard more || cursor.isZero else {
            throw WireFault.invalidCombination(
                "QueryCatalog response: next cursor without the more flag")
        }
        // §8.2: the cursor's CRC-32 covers the StoreId that minted it followed by the cursor's own
        // first twelve bytes. A *request* cursor is unverifiable here because the frame does not
        // carry the store it was scoped to, but a page reports that StoreId itself — so this one is
        // verifiable and is verified. Skipping it would mean following a corrupted or foreign cursor
        // into a page this reader cannot interpret.
        if !cursor.isZero, cursor.checksum != cursor.expectedChecksum(storeId: store) {
            throw WireFault.cursorChecksum(
                "QueryCatalog response: next cursor CRC \(cursor.checksum) under this page's StoreId")
        }
        // §8.2: "A page returns only as many whole entries as fit the negotiated control frame, and
        // never more than ten."
        guard entryCount <= 10 else {
            throw WireFault.invalidCombination("QueryCatalog response: \(entryCount) entries")
        }

        var entries: [CatalogEntry] = []
        for _ in 0..<entryCount {
            let logicalId = LogicalObjectId(try reader.u64())
            let entryRevision = Revision(try reader.u64())
            let length = try reader.u64()
            let crc = try reader.u32()
            let entryFlags = try reader.u16()
            guard entryFlags == 0 else {
                throw WireFault.reservedBits("QueryCatalog response: entry flags are zero in v3.0")
            }
            let metadataLength = Int(try reader.u16())
            try reader.reserved(4, "QueryCatalog response: entry offset 32")
            guard metadataLength <= WireLimits.catalogMetadataCeiling else {
                throw WireFault.nestedLength(
                    "QueryCatalog response: metadata length \(metadataLength)")
            }
            let envelopeBytes = Array(try reader.take(metadataLength))
            let envelope = try MetadataEnvelope.decode(
                envelopeBytes,
                maximumEncodedLength: SchemaClass.catalogProjection.envelopeCeiling)
            try envelope.validated(kind: kind, schemaClass: .catalogProjection, mutating: false)
            entries.append(
                CatalogEntry(
                    logicalObjectId: logicalId, revision: entryRevision, length: length, crc32: crc,
                    metadata: envelope))
        }
        try reader.requireExhausted("the last catalog entry")
        // §8.2: "Entries are ordered by LogicalObjectId."
        for (previous, next) in zip(entries, entries.dropFirst()) {
            guard previous.logicalObjectId.rawValue < next.logicalObjectId.rawValue else {
                throw WireFault.invalidCombination("QueryCatalog response: entries out of order")
            }
        }
        return CatalogPage(
            storeId: store, objectKind: kind, repositoryRevision: revision, nextCursor: cursor,
            entries: entries)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.raw(storeId.bytes)
        writer.u16(objectKind.rawValue)
        writer.u16(try narrowU16(entries.count, "QueryCatalog response: entry count"))
        writer.u64(repositoryRevision.rawValue)
        writer.raw(nextCursor.bytes)
        for entry in entries {
            writer.u64(entry.logicalObjectId.rawValue)
            writer.u64(entry.revision.rawValue)
            writer.u64(entry.length)
            writer.u32(entry.crc32)
            writer.u16(0)
            try requireAtMost(
                entry.metadata.encodedLength, WireLimits.catalogMetadataCeiling,
                "QueryCatalog response: entry metadata")
            writer.u16(
                try narrowU16(entry.metadata.encodedLength, "QueryCatalog response: metadata length"))
            writer.zeros(4)
            writer.raw(try entry.metadata.encoded())
        }
        return writer.bytes
    }
}

// MARK: - QueryDraft (§8.3)

public struct QueryDraftRequest: Hashable, Sendable {
    public struct Flags: OptionSet, Sendable, Hashable {
        public let rawValue: UInt16
        public init(rawValue: UInt16) { self.rawValue = rawValue }
        public static let expectedRevision = Flags(rawValue: 1 << 0)
        public static let cursor = Flags(rawValue: 1 << 1)
        static let defined: UInt16 = 0x0003
    }

    public let parentOperationId: OperationId
    public let flags: Flags
    public let requestedLimit: UInt8
    public let expectedDraftRevision: DraftRevision
    public let cursor: CatalogCursor

    public static func decode(_ bytes: [UInt8]) throws -> QueryDraftRequest {
        try requireExactPayload(bytes.count, 44, "QueryDraft")
        var reader = ByteReader(bytes, subject: "QueryDraft")
        let parent = OperationId(unchecked: try reader.opaque16())
        let flagsRaw = try reader.u16()
        guard flagsRaw & ~Flags.defined == 0 else {
            throw WireFault.reservedBits("QueryDraft: flags \(flagsRaw)")
        }
        let flags = Flags(rawValue: flagsRaw)
        let limit = try reader.u8()
        try reader.reserved(1, "QueryDraft offset 19")
        let revision = DraftRevision(try reader.u64())
        let cursor = CatalogCursor(unchecked: try reader.opaque16())

        // §8.3: "requested limit, 1 through 6".
        guard limit >= 1, limit <= 6 else {
            throw WireFault.invalidCombination("QueryDraft: requested limit \(limit)")
        }
        if !flags.contains(.expectedRevision) {
            guard revision.rawValue == 0 else {
                throw WireFault.reservedBits("QueryDraft: expected revision without its flag")
            }
        }
        if flags.contains(.cursor) {
            guard flags.contains(.expectedRevision) else {
                throw WireFault.invalidCombination("QueryDraft: cursor flag alone")
            }
            guard cursor.revision.rawValue == revision.rawValue else {
                throw WireFault.invalidCombination("QueryDraft: cursor revision mismatch")
            }
            // §8.3: the cursor's third field is zero, not an ObjectKind.
            guard cursor.objectKindCode == 0 else {
                throw WireFault.reservedBits("QueryDraft: cursor kind field is zero")
            }
        } else {
            guard cursor.isZero else {
                throw WireFault.reservedBits("QueryDraft: cursor bytes without the cursor flag")
            }
        }
        return QueryDraftRequest(
            parentOperationId: parent, flags: flags, requestedLimit: limit,
            expectedDraftRevision: revision, cursor: cursor)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.raw(parentOperationId.bytes)
        writer.u16(flags.rawValue)
        writer.u8(requestedLimit)
        writer.zeros(1)
        writer.u64(expectedDraftRevision.rawValue)
        writer.raw(cursor.bytes)
        return writer.bytes
    }
}

/// §8.3's draft-part states.
public enum DraftPartState: UInt8, Sendable, CaseIterable {
    case prepared = 0
    case streaming = 1
    case sealed = 2
    case aborted = 3
}

/// One 68-byte QueryDraft entry.
public struct DraftEntry: Hashable, Sendable {
    public static let entryBytes = 68

    public let childOperationId: OperationId
    /// §8.3: zero unless the state is sealed.
    public let draftPartRef: DraftPartRef
    public let draftPartKind: DraftPartKind
    public let partKey: PartKey
    public let state: DraftPartState
    public let durableOffset: UInt64
    public let declaredLength: UInt64
    public let crc32: UInt32
}

public struct DraftPage: Hashable, Sendable {
    public struct Flags: OptionSet, Sendable, Hashable {
        public let rawValue: UInt8
        public init(rawValue: UInt8) { self.rawValue = rawValue }
        public static let manifestStreaming = Flags(rawValue: 1 << 0)
        public static let aborting = Flags(rawValue: 1 << 1)
        static let defined: UInt8 = 0x03
    }

    public let parentOperationId: OperationId
    public let draftRevision: DraftRevision
    public let nextCursor: CatalogCursor
    public let flags: Flags
    public let entries: [DraftEntry]

    public static func decode(_ bytes: [UInt8], more: Bool) throws -> DraftPage {
        guard bytes.count >= 44 else {
            throw WireFault.truncated("QueryDraft response: \(bytes.count) bytes")
        }
        var reader = ByteReader(bytes, subject: "QueryDraft response")
        let parent = OperationId(unchecked: try reader.opaque16())
        let revision = DraftRevision(try reader.u64())
        let cursor = CatalogCursor(unchecked: try reader.opaque16())
        let count = Int(try reader.u8())
        let flagsRaw = try reader.u8()
        guard flagsRaw & ~Flags.defined == 0 else {
            throw WireFault.reservedBits("QueryDraft response: flags \(flagsRaw)")
        }
        try reader.reserved(2, "QueryDraft response offset 42")
        guard more || cursor.isZero else {
            throw WireFault.invalidCombination(
                "QueryDraft response: next cursor without the more flag")
        }
        // §8.3: "Up to six 68-byte entries follow."
        guard count <= 6 else {
            throw WireFault.invalidCombination("QueryDraft response: \(count) entries")
        }

        var entries: [DraftEntry] = []
        for _ in 0..<count {
            let child = OperationId(unchecked: try reader.opaque16())
            let ref = DraftPartRef(unchecked: try reader.opaque16())
            let kindRaw = try reader.u16()
            guard let kind = DraftPartKind(rawValue: kindRaw) else {
                throw WireFault.unknownEnum("QueryDraft response: DraftPartKind \(kindRaw)")
            }
            try reader.reserved(2, "QueryDraft response: entry offset 34")
            let key = PartKey(try reader.u64())
            let stateRaw = try reader.u8()
            guard let state = DraftPartState(rawValue: stateRaw) else {
                throw WireFault.unknownEnum("QueryDraft response: state \(stateRaw)")
            }
            // §8.3: the entry flags byte "is an inactive fixed-width alternative under Section 1".
            try reader.reserved(1, "QueryDraft response: entry flags")
            try reader.reserved(2, "QueryDraft response: entry offset 46")
            let offset = try reader.u64()
            let length = try reader.u64()
            let crc = try reader.u32()
            guard state == .sealed || ref.isZero else {
                throw WireFault.invalidCombination(
                    "QueryDraft response: DraftPartRef on a non-sealed entry")
            }
            entries.append(
                DraftEntry(
                    childOperationId: child, draftPartRef: ref, draftPartKind: kind, partKey: key,
                    state: state, durableOffset: offset, declaredLength: length, crc32: crc))
        }
        try reader.requireExhausted("the last draft entry")
        // §8.3: "Entries are strictly ordered by (DraftPartKind, part key)."
        for (previous, next) in zip(entries, entries.dropFirst()) {
            let a = (previous.draftPartKind.rawValue, previous.partKey.rawValue)
            let b = (next.draftPartKind.rawValue, next.partKey.rawValue)
            guard a < b else {
                throw WireFault.invalidCombination("QueryDraft response: entries out of order")
            }
        }
        return DraftPage(
            parentOperationId: parent, draftRevision: revision, nextCursor: cursor,
            flags: Flags(rawValue: flagsRaw), entries: entries)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.raw(parentOperationId.bytes)
        writer.u64(draftRevision.rawValue)
        writer.raw(nextCursor.bytes)
        writer.u8(try narrowU8(entries.count, "QueryDraft response: entry count"))
        writer.u8(flags.rawValue)
        writer.zeros(2)
        for entry in entries {
            writer.raw(entry.childOperationId.bytes)
            writer.raw(entry.draftPartRef.bytes)
            writer.u16(entry.draftPartKind.rawValue)
            writer.zeros(2)
            writer.u64(entry.partKey.rawValue)
            writer.u8(entry.state.rawValue)
            writer.u8(0)
            writer.zeros(2)
            writer.u64(entry.durableOffset)
            writer.u64(entry.declaredLength)
            writer.u32(entry.crc32)
        }
        return writer.bytes
    }
}

// MARK: - QueryWeatherRequest (§8.4)

/// §8.4's 96-byte weather request context. Coordinates are signed degrees times 10,000,000 — a
/// different scale from the volume manifest's microdegrees, and no decoder infers one from the other.
public struct WeatherRequestContext: Hashable, Sendable {
    public enum State: UInt8, Sendable, CaseIterable {
        case pending = 1
        case satisfied = 2
    }

    public let storeId: StoreId
    public let currentWeatherRequestId: WeatherRequestId
    public let requestContextRevision: UInt64
    public let headPresent: Bool
    /// The store-owned singleton identity. Clients never choose, derive, or reject it — including
    /// zero, which is an allocated identity like any other.
    public let weatherLogicalObjectId: LogicalObjectId
    public let weatherRepositoryRevision: Revision
    public let headWeatherRequestId: WeatherRequestId
    public let centreLatitudeE7: Int32
    public let centreLongitudeE7: Int32
    public let requiredRadiusMetres: UInt32
    public let earliestIssuedUTC: Int64
    public let requiredValidUntilUTC: Int64
    public let state: State

    public static func decode(_ bytes: [UInt8]) throws -> WeatherRequestContext {
        try requireExactPayload(bytes.count, 96, "QueryWeatherRequest response")
        var reader = ByteReader(bytes, subject: "QueryWeatherRequest response")
        let store = StoreId(unchecked: try reader.opaque16())
        let currentId = WeatherRequestId(try reader.u64())
        let contextRevision = try reader.u64()
        let flags = try reader.u32()
        guard flags & ~UInt32(1) == 0 else {
            throw WireFault.reservedBits("QueryWeatherRequest response: flags \(flags)")
        }
        let headPresent = flags & 1 == 1
        let logicalId = LogicalObjectId(try reader.u64())
        let repositoryRevision = Revision(try reader.u64())
        let headId = WeatherRequestId(try reader.u64())
        guard headPresent || headId.rawValue == 0 else {
            throw WireFault.reservedBits(
                "QueryWeatherRequest response: head request ID with head-present clear")
        }
        let latitude = try reader.i32()
        let longitude = try reader.i32()
        let radius = try reader.u32()
        let issued = try reader.i64()
        let validUntil = try reader.i64()
        let stateRaw = try reader.u8()
        guard let state = State(rawValue: stateRaw) else {
            throw WireFault.unknownEnum("QueryWeatherRequest response: context state \(stateRaw)")
        }
        try reader.reserved(7, "QueryWeatherRequest response offset 89")

        // Registries §3's ranges.
        guard latitude >= -900_000_000, latitude <= 900_000_000 else {
            throw WireFault.invalidCombination("QueryWeatherRequest response: latitude \(latitude)")
        }
        guard longitude >= -1_800_000_000, longitude <= 1_800_000_000 else {
            throw WireFault.invalidCombination("QueryWeatherRequest response: longitude \(longitude)")
        }
        guard radius != 0, radius <= 100_000 else {
            throw WireFault.invalidCombination("QueryWeatherRequest response: radius \(radius)")
        }
        guard validUntil > issued else {
            throw WireFault.invalidCombination(
                "QueryWeatherRequest response: valid-until is not later than earliest issued")
        }
        return WeatherRequestContext(
            storeId: store, currentWeatherRequestId: currentId,
            requestContextRevision: contextRevision, headPresent: headPresent,
            weatherLogicalObjectId: logicalId, weatherRepositoryRevision: repositoryRevision,
            headWeatherRequestId: headId, centreLatitudeE7: latitude, centreLongitudeE7: longitude,
            requiredRadiusMetres: radius, earliestIssuedUTC: issued,
            requiredValidUntilUTC: validUntil, state: state)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.raw(storeId.bytes)
        writer.u64(currentWeatherRequestId.rawValue)
        writer.u64(requestContextRevision)
        writer.u32(headPresent ? 1 : 0)
        writer.u64(weatherLogicalObjectId.rawValue)
        writer.u64(weatherRepositoryRevision.rawValue)
        writer.u64(headWeatherRequestId.rawValue)
        writer.i32(centreLatitudeE7)
        writer.i32(centreLongitudeE7)
        writer.u32(requiredRadiusMetres)
        writer.i64(earliestIssuedUTC)
        writer.i64(requiredValidUntilUTC)
        writer.u8(state.rawValue)
        writer.zeros(7)
        return writer.bytes
    }
}
