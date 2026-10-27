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

    case map = 5
    case retiredMapSet = 6
    case update = 7
    case rollbackReserve = 8
    case metadata = 9
}

public enum Opcode: UInt8, CaseIterable, Hashable, Sendable {
    case list = 1
    case status = 2
    case get = 3
    case put = 4
    case remove = 5
    case cancel = 6
    case arm = 7
    case format = 8
    case archiveRide = 9
}

public struct CatalogFlags: OptionSet, Hashable, Sendable {
    public let rawValue: UInt16
    public init(rawValue: UInt16) { self.rawValue = rawValue }

    public static let recording = Self(rawValue: 1 << 0)
    public static let retained = Self(rawValue: 1 << 1)
    public static let reserved = Self(rawValue: 1 << 2)
    public static let assistantAccepted = Self(rawValue: 1 << 3)
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

public struct ArmResult: Hashable, Sendable {
    public let rollbackObjectID: ObjectID
    public let commitSequence: UInt64
}

public struct ArchiveRideResult: Hashable, Sendable {
    public let commitSequence: UInt64
    public let timestamp: UInt32
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

/// A request control record, validated before it reaches the radio. The Rust client builds the
/// requests and decodes the responses.
public struct ControlFrame: Hashable, Sendable {
    public let opcode: Opcode
    public let requestID: RequestID
    public let payload: Data

    public init(decodingRequest record: Data) throws {
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
        guard flags == 0 else { throw WireError.invalidFlags }

        let payload = try cursor.read(count: payloadLength)
        try Self.validate(payload, opcode: opcode)
        self.opcode = opcode
        self.requestID = requestID
        self.payload = payload
    }

    private static func validate(_ payload: Data, opcode: Opcode) throws {
        let length = switch opcode {
        case .list, .format: 32
        case .status, .get, .remove, .arm: 16
        case .archiveRide: 44
        case .put: 84
        case .cancel: 4
        }
        guard payload.count == length else { throw WireError.invalidLength }

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
            guard kind != .ride, kind != .rollbackReserve, kind != .metadata else { throw WireError.invalidCombination }
            let requestFlags = try c.u16()
            guard requestFlags == 0 else { throw WireError.invalidFlags }
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
        case .archiveRide:
            guard try c.read(count: 16).contains(where: { $0 != 0 }),
                  try c.u64() != 0, try c.u64() != 0, try c.u64() != 0 else {
                throw WireError.invalidCombination
            }
        case .format:
            let expected = try c.read(count: 16)
            let replacement = try c.read(count: 16)
            guard replacement.contains(where: { $0 != 0 }), replacement != expected else {
                throw WireError.invalidCombination
            }
        }
    }
}

public struct StreamRecord: Hashable, Sendable {
    public let requestID: RequestID
    public let offset: UInt64
    public let payload: Data

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
