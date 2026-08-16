import Foundation

/// §13's stream direction.
public enum StreamDirection: UInt8, Sendable, CaseIterable {
    case upload = 1
    case download = 2
    case status = 3

    var isData: Bool { self == .upload || self == .download }
}

/// §13's stream flags.
public struct StreamFlags: OptionSet, Sendable, Hashable {
    public let rawValue: UInt8
    public init(rawValue: UInt8) { self.rawValue = rawValue }
    public static let fault = StreamFlags(rawValue: 1 << 0)
    public static let terminal = StreamFlags(rawValue: 1 << 1)
    static let defined: UInt8 = 0x03
}

/// §13's fault disposition.
public enum StreamFaultDisposition: UInt8, Sendable, CaseIterable {
    case resumeWithNewSession = 0
    case operationDurablyAborted = 1
    case streamTransportClosed = 2

    /// §13's exhaustive table: disposition `0` is the nonterminal form (fault bit alone) and `1`
    /// and `2` are terminal (fault and terminal bits together).
    var isTerminal: Bool { self != .resumeWithNewSession }
}

/// §13's 24-byte fault status body.
public struct StreamFault: Hashable, Sendable {
    public static let bodyBytes = 24

    public let category: ErrorCategory
    public let detail: UInt16
    public let expectedNextOffset: UInt64
    public let durableNextOffset: UInt64
    public let disposition: StreamFaultDisposition

    static func decode(_ reader: inout ByteReader, terminal: Bool) throws -> StreamFault {
        let categoryRaw = try reader.u16()
        guard let category = ErrorCategory(rawValue: categoryRaw), category.isStreamFaultCategory
        else {
            // §13: "Only namespace-zero transport category/details from Section 12 are valid in
            // this compact body; semantic/domain errors use a correlated control response."
            throw WireFault.unknownEnum("stream fault: category \(categoryRaw)")
        }
        let detail = try reader.u16()
        let expected = try reader.u64()
        let durable = try reader.u64()
        let dispositionRaw = try reader.u8()
        guard let disposition = StreamFaultDisposition(rawValue: dispositionRaw) else {
            throw WireFault.unknownEnum("stream fault: disposition \(dispositionRaw)")
        }
        try reader.reserved(3, "stream fault offset 21")
        guard disposition.isTerminal == terminal else {
            throw WireFault.invalidCombination(
                "stream fault: disposition \(dispositionRaw) with terminal \(terminal)")
        }
        return StreamFault(
            category: category, detail: detail, expectedNextOffset: expected,
            durableNextOffset: durable, disposition: disposition)
    }

    func encode(into writer: inout ByteWriter) {
        writer.u16(category.rawValue)
        writer.u16(detail)
        writer.u64(expectedNextOffset)
        writer.u64(durableNextOffset)
        writer.u8(disposition.rawValue)
        writer.zeros(3)
    }
}

/// One §13 stream transport record: a 16-byte header and its payload. Every data frame carries a
/// SessionId and an absolute offset.
public struct StreamFrame: Hashable, Sendable {
    public enum Body: Hashable, Sendable {
        case data([UInt8])
        case fault(StreamFault)
    }

    public let sessionId: SessionId
    public let absoluteOffset: UInt64
    public let direction: StreamDirection
    public let flags: StreamFlags
    public let body: Body

    public init(
        sessionId: SessionId, absoluteOffset: UInt64, direction: StreamDirection,
        flags: StreamFlags, body: Body
    ) {
        self.sessionId = sessionId
        self.absoluteOffset = absoluteOffset
        self.direction = direction
        self.flags = flags
        self.body = body
    }

    public static func decode(_ record: [UInt8]) throws -> StreamFrame {
        guard record.count >= WireLimits.streamHeaderBytes else {
            throw WireFault.recordLength("stream record: \(record.count) bytes")
        }
        guard record.count <= WireLimits.maximumStreamFrame else {
            throw WireFault.frameBounds("stream record: \(record.count) bytes")
        }
        var reader = ByteReader(record, subject: "stream frame")
        let sessionRaw = try reader.u32()
        let offset = try reader.u64()
        let payloadLength = Int(try reader.u16())
        let directionRaw = try reader.u8()
        let flagsRaw = try reader.u8()

        guard flagsRaw & ~StreamFlags.defined == 0 else {
            throw WireFault.malformedHeader("stream frame: flags \(flagsRaw)")
        }
        let flags = StreamFlags(rawValue: flagsRaw)
        guard let direction = StreamDirection(rawValue: directionRaw) else {
            throw WireFault.unknownEnum("stream frame: direction \(directionRaw)")
        }
        guard let session = SessionId(sessionRaw) else {
            throw WireFault.unknownEnum("stream frame: zero SessionId")
        }
        guard record.count == WireLimits.streamHeaderBytes + payloadLength else {
            throw WireFault.payloadLength(
                "stream frame: \(record.count) bytes for a declared \(payloadLength)")
        }

        let body: Body
        if direction.isData {
            // §13: "Data directions have nonempty payload, zero flags, and exact offset equal to
            // the session's next offset."
            guard flags.isEmpty else {
                throw WireFault.malformedHeader("stream frame: flag on a data direction")
            }
            guard payloadLength > 0 else {
                throw WireFault.payloadLength("stream frame: empty data payload")
            }
            guard offset.addingReportingOverflow(UInt64(payloadLength)).overflow == false else {
                throw WireFault.payloadLength("stream frame: offset + length overflows")
            }
            body = .data(Array(try reader.take(payloadLength)))
        } else {
            // §13's exhaustive table: `0` and terminal-alone are reserved and rejected.
            guard flags.contains(.fault) else {
                throw WireFault.malformedHeader("stream frame: status without the fault bit")
            }
            guard offset == 0 else {
                throw WireFault.reservedBits("stream frame: status direction with offset \(offset)")
            }
            guard payloadLength == StreamFault.bodyBytes else {
                throw WireFault.payloadLength("stream frame: fault body of \(payloadLength) bytes")
            }
            body = .fault(try StreamFault.decode(&reader, terminal: flags.contains(.terminal)))
        }
        try reader.requireExhausted("the stream payload")
        return StreamFrame(
            sessionId: session, absoluteOffset: offset, direction: direction, flags: flags,
            body: body)
    }

    /// Fallible for the same reason the control encoder is: §1's 4,096-byte hard maximum makes a
    /// larger record unsendable, and `offset + length` must not overflow the `u64` space a receiver
    /// checks it against. The *negotiated* (and, on BLE, the CoC-effective) limit is the transport
    /// adapter's seam, not this codec's.
    public func encoded() throws -> [UInt8] {
        var payload = ByteWriter()
        switch body {
        case .data(let bytes): payload.raw(bytes)
        case .fault(let fault): fault.encode(into: &payload)
        }
        try requireAtMost(
            WireLimits.streamHeaderBytes + payload.bytes.count, WireLimits.maximumStreamFrame,
            "stream frame")
        guard
            absoluteOffset.addingReportingOverflow(UInt64(payload.bytes.count)).overflow == false
        else {
            throw WireFault.payloadLength("stream frame: offset + length overflows")
        }
        var writer = ByteWriter()
        writer.u32(sessionId.rawValue)
        writer.u64(absoluteOffset)
        writer.u16(try narrowU16(payload.bytes.count, "stream frame: payload length"))
        writer.u8(direction.rawValue)
        writer.u8(flags.rawValue)
        writer.raw(payload.bytes)
        return writer.bytes
    }
}
