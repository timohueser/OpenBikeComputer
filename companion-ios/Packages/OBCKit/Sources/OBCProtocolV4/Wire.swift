import Foundation

public enum FlatStoreV4 {
    public static let wireMajor: UInt8 = 4
    public static let controlHeaderLength = 16
    public static let streamHeaderLength = 16
    public static let maximumDisplayNameLength = 48
}

public struct RequestID: RawRepresentable, Hashable, Sendable {
    public let rawValue: UInt32

    public init?(rawValue: UInt32) {
        guard rawValue != 0 else { return nil }
        self.rawValue = rawValue
    }
}

public struct ObjectID: RawRepresentable, Hashable, Sendable, Comparable {
    public let rawValue: UInt64
    public init(rawValue: UInt64) { self.rawValue = rawValue }
    public static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }
}

public struct Revision: RawRepresentable, Hashable, Sendable, Comparable {
    public let rawValue: UInt64
    public init(rawValue: UInt64) { self.rawValue = rawValue }
    public static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }
}

public struct StoreID: Hashable, Sendable, CustomStringConvertible {
    public let bytes: Data

    public init(bytes: Data) throws {
        guard bytes.count == 16 else { throw WireError.invalidLength }
        self.bytes = bytes
    }

    public var description: String {
        bytes.map { String(format: "%02x", $0) }.joined()
    }
}

public enum ObjectKind: UInt16, CaseIterable, Hashable, Sendable {
    case route = 1
    case trip = 2
    case ride = 3
    case weather = 4
    case map = 5
    case retiredMapSet = 6
    case update = 7
    case rollbackReserve = 8
}

public enum Opcode: UInt8, CaseIterable, Hashable, Sendable {
    case list = 1
    case status = 2
    case get = 3
    case put = 4
    case remove = 5
    case cancel = 6
    case arm = 7
}

public struct CatalogFlags: OptionSet, Hashable, Sendable {
    public let rawValue: UInt16
    public init(rawValue: UInt16) { self.rawValue = rawValue }

    public static let recording = Self(rawValue: 1 << 0)
    public static let retained = Self(rawValue: 1 << 1)
    public static let reserved = Self(rawValue: 1 << 2)
    static let known: Self = [.recording, .retained, .reserved]
}

public struct CatalogEntry: Hashable, Sendable {
    public let objectID: ObjectID
    public let revision: Revision
    public let payloadLength: UInt64
    public let payloadCRC32: UInt32
    public let kind: ObjectKind
    public let flags: CatalogFlags
    public let displayName: String
}

public struct CatalogPage: Hashable, Sendable {
    public let storeID: StoreID
    public let commitSequence: UInt64
    public let entries: [CatalogEntry]
    public let hasMore: Bool
}

public enum StatusState: UInt8, Hashable, Sendable {
    case absent = 0
    case committed = 1
    case superseded = 2
}

public struct StatusResult: Hashable, Sendable {
    public let state: StatusState
    public let headRevision: Revision
    public let headPayloadLength: UInt64
    public let headPayloadCRC32: UInt32
}

public struct GetResult: Hashable, Sendable {
    public let revision: Revision
    public let payloadLength: UInt64
    public let payloadCRC32: UInt32
}

public struct PutResult: Hashable, Sendable {
    public let objectID: ObjectID
    public let revision: Revision
    public let payloadLength: UInt64
    public let payloadCRC32: UInt32
}

public struct RemoveResult: Hashable, Sendable {
    /// `nil` when a lost response was reconciled as an absent object with STATUS.
    public let commitSequence: UInt64?
}

public enum CancelResult: UInt8, Hashable, Sendable {
    case cancelled = 0
    case noSuchTransfer = 1
}

public struct ArmResult: Hashable, Sendable {
    public let rollbackObjectID: ObjectID
    public let commitSequence: UInt64
}

public enum RemoteErrorCode: UInt16, Hashable, Sendable {
    case unsupported = 1
    case invalidFrame = 2
    case invalidRequest = 3
    case notFound = 4
    case revisionConflict = 5
    case noSpace = 6
    case checksumFailure = 7
    case mediaIO = 8
    case busy = 9
    case cancelled = 10
    case rejected = 11
    case `internal` = 12
    case catalogChanged = 13
    case readOnly = 14
}

public struct RemoteErrorBody: Error, Hashable, Sendable {
    public let code: RemoteErrorCode
    public let detail: UInt16
    public let context: UInt64
}

public enum WireError: Error, Equatable, Sendable {
    case truncated
    case trailingBytes
    case invalidMagic
    case unsupportedMajor(UInt8)
    case unsupportedOpcode(UInt8)
    case invalidFlags
    case invalidLength
    case invalidReserved
    case invalidRequestID
    case invalidEnum
    case invalidCombination
    case invalidUTF8
    case remote(RemoteErrorBody)
}

public enum ControlDirection: Equatable, Sendable {
    case request
    case response
}

public struct ControlFrame: Hashable, Sendable {
    public static let responseFlag: UInt16 = 1 << 0
    public static let errorFlag: UInt16 = 1 << 1
    public static let moreFlag: UInt16 = 1 << 2

    public let opcode: Opcode
    public let flags: UInt16
    public let requestID: RequestID
    public let payload: Data

    public init(opcode: Opcode, flags: UInt16, requestID: RequestID, payload: Data) {
        self.opcode = opcode
        self.flags = flags
        self.requestID = requestID
        self.payload = payload
    }

    public var isError: Bool { flags & Self.errorFlag != 0 }
    public var hasMore: Bool { flags & Self.moreFlag != 0 }

    public func encode() -> Data {
        var out = Data("OBC4".utf8)
        out.append(FlatStoreV4.wireMajor)
        out.append(opcode.rawValue)
        out.appendLE(flags)
        out.appendLE(UInt16(payload.count))
        out.appendLE(UInt16(0))
        out.appendLE(requestID.rawValue)
        out.append(payload)
        return out
    }

    public init(decoding record: Data, direction: ControlDirection) throws {
        guard record.count >= FlatStoreV4.controlHeaderLength else { throw WireError.truncated }
        var cursor = ByteCursor(record)
        guard try cursor.read(count: 4) == Data("OBC4".utf8) else { throw WireError.invalidMagic }
        let major = try cursor.u8()
        guard major == FlatStoreV4.wireMajor else { throw WireError.unsupportedMajor(major) }
        let opcodeByte = try cursor.u8()
        guard let opcode = Opcode(rawValue: opcodeByte) else { throw WireError.unsupportedOpcode(opcodeByte) }
        let flags = try cursor.u16()
        let payloadLength = Int(try cursor.u16())
        guard try cursor.u16() == 0 else { throw WireError.invalidReserved }
        guard let requestID = RequestID(rawValue: try cursor.u32()) else { throw WireError.invalidRequestID }
        let expectedLength = FlatStoreV4.controlHeaderLength + payloadLength
        guard record.count >= expectedLength else { throw WireError.truncated }
        guard record.count == expectedLength else { throw WireError.trailingBytes }

        switch direction {
        case .request:
            guard flags == 0 else { throw WireError.invalidFlags }
        case .response:
            guard flags & Self.responseFlag != 0, flags & ~UInt16(0x0007) == 0 else {
                throw WireError.invalidFlags
            }
            if flags & Self.errorFlag != 0 {
                guard flags == Self.responseFlag | Self.errorFlag else { throw WireError.invalidFlags }
            } else if flags & Self.moreFlag != 0, opcode != .list {
                throw WireError.invalidFlags
            }
        }

        let payload = try cursor.read(count: payloadLength)
        try Self.validate(payload: payload, opcode: opcode, direction: direction, flags: flags)
        self.init(opcode: opcode, flags: flags, requestID: requestID, payload: payload)
    }

    private static func validate(
        payload: Data, opcode: Opcode, direction: ControlDirection, flags: UInt16
    ) throws {
        if flags & errorFlag != 0 {
            guard payload.count == 16 else { throw WireError.invalidLength }
            _ = try decodeRemoteError(payload)
            return
        }

        let expected: Int
        switch (direction, opcode) {
        case (.request, .list): expected = 32
        case (.request, .status), (.request, .get), (.request, .remove), (.request, .arm): expected = 16
        case (.request, .put): expected = 84
        case (.request, .cancel): expected = 4
        case (.response, .list):
            guard payload.count >= 24, (payload.count - 24) % 88 == 0 else { throw WireError.invalidLength }
            _ = try decodeCatalogPage(payload, hasMore: flags & moreFlag != 0)
            return
        case (.response, .status), (.response, .get): expected = 24
        case (.response, .put): expected = 32
        case (.response, .remove): expected = 8
        case (.response, .cancel): expected = 1
        case (.response, .arm): expected = 16
        }
        guard payload.count == expected else { throw WireError.invalidLength }

        if direction == .request { try validateRequestPayload(payload, opcode: opcode) }
    }

    private static func validateRequestPayload(_ payload: Data, opcode: Opcode) throws {
        var c = ByteCursor(payload)
        switch opcode {
        case .list:
            let kind = try c.u16()
            if kind != 0, ObjectKind(rawValue: kind) == nil { throw WireError.invalidEnum }
            let flags = try c.u16()
            guard flags & ~UInt16(1) == 0 else { throw WireError.invalidFlags }
            guard try c.u32() == 0 else { throw WireError.invalidReserved }
            let object = try c.u64(), revision = try c.u64(), sequence = try c.u64()
            if flags == 0 {
                guard object == 0, revision == 0, sequence == 0 else {
                    throw WireError.invalidCombination
                }
            } else {
                // A freshly initialized catalog can legitimately page at commit sequence zero.
                // Object identities and revisions are never zero; the sequence is the exact value
                // returned by the first page, including zero.
                guard object != 0, revision != 0 else { throw WireError.invalidCombination }
            }
        case .status:
            guard try c.u64() != 0, try c.u64() != 0 else { throw WireError.invalidCombination }
        case .get:
            guard try c.u64() != 0 else { throw WireError.invalidCombination }
        case .put:
            let object = try c.u64(), revision = try c.u64()
            _ = try c.u64(); _ = try c.u32()
            let kindRaw = try c.u16()
            guard let kind = ObjectKind(rawValue: kindRaw) else { throw WireError.invalidEnum }
            guard kind != .ride, kind != .rollbackReserve else { throw WireError.invalidCombination }
            let requestFlags = try c.u16()
            guard requestFlags & ~UInt16(1) == 0 else { throw WireError.invalidFlags }
            guard requestFlags == 0 || kind == .weather else { throw WireError.invalidCombination }
            let nameLength = Int(try c.u8())
            guard nameLength <= FlatStoreV4.maximumDisplayNameLength else { throw WireError.invalidCombination }
            guard try c.read(count: 3).allSatisfy({ $0 == 0 }) else { throw WireError.invalidReserved }
            let nameField = try c.read(count: 48)
            guard nameField.dropFirst(nameLength).allSatisfy({ $0 == 0 }) else { throw WireError.invalidReserved }
            guard String(data: nameField.prefix(nameLength), encoding: .utf8) != nil else { throw WireError.invalidUTF8 }
            guard (object == 0 && revision == 0) || (object != 0 && revision != 0) else {
                throw WireError.invalidCombination
            }
        case .remove, .arm:
            guard try c.u64() != 0, try c.u64() != 0 else { throw WireError.invalidCombination }
        case .cancel:
            guard try c.u32() != 0 else { throw WireError.invalidCombination }
        }
    }
}

public enum ControlRequest: Hashable, Sendable {
    case list(kind: ObjectKind?, cursor: CatalogCursor?)
    case status(objectID: ObjectID, revision: Revision)
    case get(objectID: ObjectID, revision: Revision?)
    case put(PutRequest)
    case remove(objectID: ObjectID, expectedRevision: Revision)
    case cancel(transfer: RequestID)
    case arm(packageObjectID: ObjectID, expectedRevision: Revision)

    public func frame(requestID: RequestID) throws -> ControlFrame {
        var payload = Data()
        let opcode: Opcode
        switch self {
        case .list(let kind, let cursor):
            opcode = .list
            payload.appendLE(kind?.rawValue ?? 0)
            payload.appendLE(UInt16(cursor == nil ? 0 : 1))
            payload.appendLE(UInt32(0))
            payload.appendLE(cursor?.objectID.rawValue ?? 0)
            payload.appendLE(cursor?.revision.rawValue ?? 0)
            payload.appendLE(cursor?.commitSequence ?? 0)
        case .status(let objectID, let revision):
            guard objectID.rawValue != 0, revision.rawValue != 0 else { throw WireError.invalidCombination }
            opcode = .status
            payload.appendLE(objectID.rawValue); payload.appendLE(revision.rawValue)
        case .get(let objectID, let revision):
            guard objectID.rawValue != 0 else { throw WireError.invalidCombination }
            opcode = .get
            payload.appendLE(objectID.rawValue); payload.appendLE(revision?.rawValue ?? 0)
        case .put(let request):
            opcode = .put
            payload = try request.encode()
        case .remove(let objectID, let expectedRevision):
            guard objectID.rawValue != 0, expectedRevision.rawValue != 0 else { throw WireError.invalidCombination }
            opcode = .remove
            payload.appendLE(objectID.rawValue); payload.appendLE(expectedRevision.rawValue)
        case .cancel(let transfer):
            opcode = .cancel
            payload.appendLE(transfer.rawValue)
        case .arm(let objectID, let expectedRevision):
            guard objectID.rawValue != 0, expectedRevision.rawValue != 0 else { throw WireError.invalidCombination }
            opcode = .arm
            payload.appendLE(objectID.rawValue); payload.appendLE(expectedRevision.rawValue)
        }
        return ControlFrame(opcode: opcode, flags: 0, requestID: requestID, payload: payload)
    }
}

public struct CatalogCursor: Hashable, Sendable {
    public let objectID: ObjectID
    public let revision: Revision
    public let commitSequence: UInt64

    public init(objectID: ObjectID, revision: Revision, commitSequence: UInt64) {
        self.objectID = objectID
        self.revision = revision
        self.commitSequence = commitSequence
    }
}

public struct PutRequest: Hashable, Sendable {
    public let objectID: ObjectID?
    public let expectedRevision: Revision?
    public let payloadLength: UInt64
    public let payloadCRC32: UInt32
    public let kind: ObjectKind
    public let retainPrevious: Bool
    public let displayName: String

    public init(
        objectID: ObjectID? = nil, expectedRevision: Revision? = nil,
        payloadLength: UInt64, payloadCRC32: UInt32, kind: ObjectKind,
        retainPrevious: Bool = false, displayName: String
    ) {
        self.objectID = objectID
        self.expectedRevision = expectedRevision
        self.payloadLength = payloadLength
        self.payloadCRC32 = payloadCRC32
        self.kind = kind
        self.retainPrevious = retainPrevious
        self.displayName = displayName
    }

    fileprivate func encode() throws -> Data {
        let name = Data(displayName.utf8)
        guard name.count <= FlatStoreV4.maximumDisplayNameLength else { throw WireError.invalidCombination }
        guard (objectID == nil && expectedRevision == nil)
            || (objectID?.rawValue ?? 0) != 0 && (expectedRevision?.rawValue ?? 0) != 0
        else { throw WireError.invalidCombination }
        guard kind != .ride, kind != .rollbackReserve else { throw WireError.invalidCombination }
        guard !retainPrevious || kind == .weather else { throw WireError.invalidCombination }

        var out = Data()
        out.appendLE(objectID?.rawValue ?? 0)
        out.appendLE(expectedRevision?.rawValue ?? 0)
        out.appendLE(payloadLength)
        out.appendLE(payloadCRC32)
        out.appendLE(kind.rawValue)
        out.appendLE(UInt16(retainPrevious ? 1 : 0))
        out.append(UInt8(name.count))
        out.append(contentsOf: [0, 0, 0])
        out.append(name)
        out.append(Data(repeating: 0, count: 48 - name.count))
        return out
    }
}

public enum ControlResponse: Hashable, Sendable {
    case list(CatalogPage)
    case status(StatusResult)
    case get(GetResult)
    case put(PutResult)
    case remove(RemoveResult)
    case cancel(CancelResult)
    case arm(ArmResult)

    public init(decoding record: Data, expectedOpcode: Opcode? = nil, expectedRequestID: RequestID? = nil) throws {
        let frame = try ControlFrame(decoding: record, direction: .response)
        if let expectedOpcode, frame.opcode != expectedOpcode { throw WireError.invalidCombination }
        if let expectedRequestID, frame.requestID != expectedRequestID { throw WireError.invalidCombination }
        if frame.isError { throw WireError.remote(try decodeRemoteError(frame.payload)) }

        var c = ByteCursor(frame.payload)
        switch frame.opcode {
        case .list:
            self = .list(try decodeCatalogPage(frame.payload, hasMore: frame.hasMore))
        case .status:
            guard let state = StatusState(rawValue: try c.u8()) else { throw WireError.invalidEnum }
            guard try c.read(count: 3).allSatisfy({ $0 == 0 }) else { throw WireError.invalidReserved }
            let result = StatusResult(
                state: state, headRevision: Revision(rawValue: try c.u64()),
                headPayloadLength: try c.u64(), headPayloadCRC32: try c.u32())
            if state == .absent {
                guard result.headRevision.rawValue == 0, result.headPayloadLength == 0,
                    result.headPayloadCRC32 == 0 else { throw WireError.invalidCombination }
            } else if result.headRevision.rawValue == 0 {
                throw WireError.invalidCombination
            }
            self = .status(result)
        case .get:
            let result = GetResult(
                revision: Revision(rawValue: try c.u64()), payloadLength: try c.u64(),
                payloadCRC32: try c.u32())
            guard try c.u32() == 0, result.revision.rawValue != 0 else { throw WireError.invalidReserved }
            self = .get(result)
        case .put:
            let result = PutResult(
                objectID: ObjectID(rawValue: try c.u64()), revision: Revision(rawValue: try c.u64()),
                payloadLength: try c.u64(), payloadCRC32: try c.u32())
            guard try c.u32() == 0, result.objectID.rawValue != 0, result.revision.rawValue != 0
            else { throw WireError.invalidCombination }
            self = .put(result)
        case .remove:
            self = .remove(RemoveResult(commitSequence: try c.u64()))
        case .cancel:
            guard let result = CancelResult(rawValue: try c.u8()) else { throw WireError.invalidEnum }
            self = .cancel(result)
        case .arm:
            let result = ArmResult(
                rollbackObjectID: ObjectID(rawValue: try c.u64()), commitSequence: try c.u64())
            guard result.rollbackObjectID.rawValue != 0 else { throw WireError.invalidCombination }
            self = .arm(result)
        }
    }
}

public struct StreamRecord: Hashable, Sendable {
    public let requestID: RequestID
    public let offset: UInt64
    public let payload: Data

    public init(requestID: RequestID, offset: UInt64, payload: Data) throws {
        guard !payload.isEmpty, payload.count <= Int(UInt16.max) else { throw WireError.invalidLength }
        self.requestID = requestID
        self.offset = offset
        self.payload = payload
    }

    public func encode() -> Data {
        var out = Data()
        out.appendLE(requestID.rawValue)
        out.appendLE(offset)
        out.appendLE(UInt16(payload.count))
        out.appendLE(UInt16(0))
        out.append(payload)
        return out
    }

    public init(decoding record: Data) throws {
        guard record.count >= FlatStoreV4.streamHeaderLength else { throw WireError.truncated }
        var c = ByteCursor(record)
        guard let requestID = RequestID(rawValue: try c.u32()) else { throw WireError.invalidRequestID }
        let offset = try c.u64()
        let payloadLength = Int(try c.u16())
        guard payloadLength != 0 else { throw WireError.invalidLength }
        guard try c.u16() == 0 else { throw WireError.invalidReserved }
        guard record.count >= FlatStoreV4.streamHeaderLength + payloadLength else { throw WireError.truncated }
        guard record.count == FlatStoreV4.streamHeaderLength + payloadLength else { throw WireError.trailingBytes }
        self.requestID = requestID
        self.offset = offset
        self.payload = try c.read(count: payloadLength)
    }
}

private func decodeRemoteError(_ data: Data) throws -> RemoteErrorBody {
    guard data.count == 16 else { throw WireError.invalidLength }
    var c = ByteCursor(data)
    guard let code = RemoteErrorCode(rawValue: try c.u16()) else { throw WireError.invalidEnum }
    let detail = try c.u16(), context = try c.u64()
    guard try c.u32() == 0 else { throw WireError.invalidReserved }
    return RemoteErrorBody(code: code, detail: detail, context: context)
}

private func decodeCatalogPage(_ data: Data, hasMore: Bool) throws -> CatalogPage {
    guard data.count >= 24, (data.count - 24) % 88 == 0 else { throw WireError.invalidLength }
    var c = ByteCursor(data)
    let storeID = try StoreID(bytes: c.read(count: 16))
    let sequence = try c.u64()
    var entries: [CatalogEntry] = []
    while c.remaining > 0 {
        let objectID = ObjectID(rawValue: try c.u64())
        let revision = Revision(rawValue: try c.u64())
        let length = try c.u64(), crc = try c.u32()
        guard let kind = ObjectKind(rawValue: try c.u16()) else { throw WireError.invalidEnum }
        let flags = CatalogFlags(rawValue: try c.u16())
        guard flags.subtracting(.known).isEmpty else { throw WireError.invalidFlags }
        let nameLength = Int(try c.u8())
        guard nameLength <= 48 else { throw WireError.invalidCombination }
        guard try c.read(count: 3).allSatisfy({ $0 == 0 }) else { throw WireError.invalidReserved }
        let nameField = try c.read(count: 48)
        guard nameField.dropFirst(nameLength).allSatisfy({ $0 == 0 }) else { throw WireError.invalidReserved }
        guard let name = String(data: nameField.prefix(nameLength), encoding: .utf8) else {
            throw WireError.invalidUTF8
        }
        guard try c.u32() == 0, objectID.rawValue != 0, revision.rawValue != 0 else {
            throw WireError.invalidReserved
        }
        entries.append(CatalogEntry(
            objectID: objectID, revision: revision, payloadLength: length,
            payloadCRC32: crc, kind: kind, flags: flags, displayName: name))
    }
    if hasMore, entries.isEmpty { throw WireError.invalidCombination }
    return CatalogPage(storeID: storeID, commitSequence: sequence, entries: entries, hasMore: hasMore)
}

private struct ByteCursor {
    private let data: Data
    private var index: Data.Index

    init(_ data: Data) { self.data = data; self.index = data.startIndex }
    var remaining: Int { data.distance(from: index, to: data.endIndex) }

    mutating func read(count: Int) throws -> Data {
        guard count >= 0, remaining >= count else { throw WireError.truncated }
        let end = data.index(index, offsetBy: count)
        defer { index = end }
        return data[index..<end]
    }

    mutating func u8() throws -> UInt8 { try read(count: 1).first! }
    mutating func u16() throws -> UInt16 { try integer(UInt16.self) }
    mutating func u32() throws -> UInt32 { try integer(UInt32.self) }
    mutating func u64() throws -> UInt64 { try integer(UInt64.self) }

    private mutating func integer<T: FixedWidthInteger>(_ type: T.Type) throws -> T {
        let bytes = try read(count: MemoryLayout<T>.size)
        return bytes.enumerated().reduce(0) { $0 | (T($1.element) << T($1.offset * 8)) }
    }
}

private extension Data {
    mutating func appendLE<T: FixedWidthInteger>(_ value: T) {
        for shift in stride(from: 0, to: T.bitWidth, by: 8) {
            append(UInt8(truncatingIfNeeded: value >> T(shift)))
        }
    }
}
