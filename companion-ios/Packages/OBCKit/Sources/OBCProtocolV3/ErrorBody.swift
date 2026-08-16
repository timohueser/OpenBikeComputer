import Foundation

/// §12's presence bits.
public struct ErrorPresence: OptionSet, Sendable, Hashable {
    public let rawValue: UInt16
    public init(rawValue: UInt16) { self.rawValue = rawValue }
    public static let retryDelay = ErrorPresence(rawValue: 1 << 0)
    public static let expectedOffset = ErrorPresence(rawValue: 1 << 1)
    public static let currentRevision = ErrorPresence(rawValue: 1 << 2)
    public static let requiredBytes = ErrorPresence(rawValue: 1 << 3)
    public static let availableBytes = ErrorPresence(rawValue: 1 << 4)
    /// §12: a durable claim exists for this OperationId under this principal.
    public static let durableClaimExists = ErrorPresence(rawValue: 1 << 5)
    /// §12: meaningful only with bit 5 — that claim is now terminal.
    public static let claimIsTerminal = ErrorPresence(rawValue: 1 << 6)
    static let defined: UInt16 = 0x007F
}

/// The 48-byte prefix plus optional non-authoritative diagnostic text of §12.
///
/// Decoding is deliberately permissive about *content*: §12's presence and guidance requirements
/// "bind senders only", so this never rejects a body because an optional field is present where it
/// expected none or absent where the category would normally require one. That permissiveness is
/// exactly what makes §11's replayed terminal bodies decodable without a special case. Only the
/// structural rules are enforced.
public struct ErrorBody: Hashable, Sendable {
    public static let prefixBytes = 48

    public let rawCategory: UInt16
    /// Detail namespace: common `0` or the affected ObjectKind for `semanticValidation`.
    public let namespace: UInt16
    public let detail: UInt16
    public let rawGuidance: UInt8
    public let rawOwner: UInt8
    public let presence: ErrorPresence
    public let retryAfterMilliseconds: UInt32
    public let expectedOffset: UInt64
    public let currentRevision: UInt64
    public let requiredBytes: UInt64
    public let availableBytes: UInt64
    /// Raw text bytes. §12: "A receiver MUST NOT reject a frame because its diagnostic text is
    /// malformed" — the bytes are kept verbatim and rendered lossily on demand.
    public let text: [UInt8]

    public var category: ErrorCategory? { ErrorCategory(rawValue: rawCategory) }
    public var guidance: RetryGuidance? { RetryGuidance(rawValue: rawGuidance) }
    public var owner: ErrorOwner? { ErrorOwner(rawValue: rawOwner) }
    public var encodedLength: Int { Self.prefixBytes + text.count }

    /// §12: rendered lossily, never parsed, matched on, or turned into behaviour.
    public var lossyText: String { String(decoding: text, as: UTF8.self) }

    /// §11: a retained Aborted replay carries both status bits and forced reject-permanently
    /// guidance. This is the discriminator a client may test to recognise one.
    public var looksLikeRetainedTerminalReplay: Bool {
        presence.contains(.durableClaimExists) && presence.contains(.claimIsTerminal)
    }

    public var fault: WireFault {
        WireFault(category ?? .internal, detail, namespace: namespace)
    }

    public init(
        rawCategory: UInt16, namespace: UInt16 = 0, detail: UInt16, rawGuidance: UInt8,
        rawOwner: UInt8 = 0, presence: ErrorPresence = [], retryAfterMilliseconds: UInt32 = 0,
        expectedOffset: UInt64 = 0, currentRevision: UInt64 = 0, requiredBytes: UInt64 = 0,
        availableBytes: UInt64 = 0, text: [UInt8] = []
    ) {
        self.rawCategory = rawCategory
        self.namespace = namespace
        self.detail = detail
        self.rawGuidance = rawGuidance
        self.rawOwner = rawOwner
        self.presence = presence
        self.retryAfterMilliseconds = retryAfterMilliseconds
        self.expectedOffset = expectedOffset
        self.currentRevision = currentRevision
        self.requiredBytes = requiredBytes
        self.availableBytes = availableBytes
        self.text = text
    }

    public static func decode(_ bytes: [UInt8]) throws -> ErrorBody {
        guard bytes.count >= prefixBytes else {
            throw WireFault.truncated("ErrorBody: \(bytes.count) bytes")
        }
        var reader = ByteReader(bytes, subject: "ErrorBody")
        let rawCategory = try reader.u16()
        let namespace = try reader.u16()
        let detail = try reader.u16()
        let rawGuidance = try reader.u8()
        let rawOwner = try reader.u8()
        let presenceRaw = try reader.u16()
        let retryAfter = try reader.u32()
        let expectedOffset = try reader.u64()
        let currentRevision = try reader.u64()
        let requiredBytes = try reader.u64()
        let availableBytes = try reader.u64()
        let textLength = Int(try reader.u8())
        try reader.reserved(1, "byte 47")

        // §12: "Category 0 is reserved and invalid. A sender never emits it and a receiver treats
        // it as a malformed body rather than as an unknown future category." A *nonzero* unknown
        // category is therefore kept, not rejected.
        guard rawCategory != 0 else { throw WireFault.unknownEnum("ErrorBody: category 0") }
        if let category = ErrorCategory(rawValue: rawCategory), category != .semanticValidation,
            namespace != 0
        {
            throw WireFault.invalidCombination(
                "ErrorBody: \(category.name) with namespace \(namespace)")
        }
        guard presenceRaw & ~ErrorPresence.defined == 0 else {
            throw WireFault.reservedBits("ErrorBody: presence bits 7…15")
        }
        let presence = ErrorPresence(rawValue: presenceRaw)
        guard !(presence.contains(.claimIsTerminal) && !presence.contains(.durableClaimExists))
        else {
            throw WireFault.invalidCombination("ErrorBody: terminal bit without the claim bit")
        }
        // §12: "Only the text length field is structural: a length above 64, or a length that
        // disagrees with the frame's payload length, is `invalidFrame` as usual."
        guard textLength <= WireLimits.errorTextCeiling else {
            throw WireFault.payloadLength("ErrorBody: text length \(textLength)")
        }
        guard reader.remaining == textLength else {
            throw WireFault.payloadLength(
                "ErrorBody: text length \(textLength), \(reader.remaining) byte(s) present")
        }
        return ErrorBody(
            rawCategory: rawCategory, namespace: namespace, detail: detail,
            rawGuidance: rawGuidance, rawOwner: rawOwner, presence: presence,
            retryAfterMilliseconds: retryAfter, expectedOffset: expectedOffset,
            currentRevision: currentRevision, requiredBytes: requiredBytes,
            availableBytes: availableBytes, text: reader.rest())
    }

    public func encoded() throws -> [UInt8] {
        try requireAtMost(text.count, WireLimits.errorTextCeiling, "ErrorBody: diagnostic text")
        var writer = ByteWriter()
        writer.u16(rawCategory)
        writer.u16(namespace)
        writer.u16(detail)
        writer.u8(rawGuidance)
        writer.u8(rawOwner)
        writer.u16(presence.rawValue)
        writer.u32(retryAfterMilliseconds)
        writer.u64(expectedOffset)
        writer.u64(currentRevision)
        writer.u64(requiredBytes)
        writer.u64(availableBytes)
        writer.u8(try narrowU8(text.count, "ErrorBody: text length"))
        writer.u8(0)
        writer.raw(text)
        return writer.bytes
    }
}
