import Foundation

/// §2's header flags. Requests have none; successful responses set `response`; errors set
/// `response|error`; `more` is valid only on a paged Capabilities, QueryCatalog, or QueryDraft
/// response.
public struct ControlFlags: OptionSet, Sendable, Hashable {
    public let rawValue: UInt16
    public init(rawValue: UInt16) { self.rawValue = rawValue }
    public static let response = ControlFlags(rawValue: 1 << 0)
    public static let error = ControlFlags(rawValue: 1 << 1)
    public static let more = ControlFlags(rawValue: 1 << 2)
    static let defined: UInt16 = 0x0007
}

/// Every decoded control payload. One case per direction per opcode, so a `switch` over this enum
/// is exhaustive over the whole v3.0 control plane.
public enum ControlBody: Hashable, Sendable {
    // Requests
    case hello(Hello)
    case startUpload(StartUploadRequest)
    case checkpointUpload(CheckpointUploadRequest)
    case finishUpload(SessionId)
    case startDownload(StartDownloadRequest)
    case finishDownload(FinishDownloadRequest)
    case abortSession(AbortSessionRequest)
    case beginDraft(BeginDraftRequest)
    case startDraftPart(StartDraftPartRequest)
    case finalizeDraft(OperationId)
    case queryOperation(OperationId)
    case queryCatalog(QueryCatalogRequest)
    case queryDraft(QueryDraftRequest)
    case queryWeatherRequest
    case deleteObject(DeleteObjectRequest)
    case setMetadata(SetMetadataRequest)
    case abortOperation(AbortOperationRequest)
    case installUpdate(KindedCommandRequest)
    case acknowledgeRideImported(KindedCommandRequest)
    case getDeviceStatus
    case getConfig
    case setConfig(DeviceConfigBlock)
    case setClock(SetClockRequest)
    case forgetBond(ForgetBondRequest)
    case echoRequest([UInt8])
    case resetStore(StoreId)

    // Responses
    case capabilities(CapabilitiesPage)
    case uploadAccepted(UploadAcceptance)
    case checkpointAccepted(CheckpointUploadResponse)
    case downloadAccepted(DownloadAcceptance)
    case downloadReleased
    case abortSessionOutcome(AbortSessionOutcome)
    case beginDraftAccepted(BeginDraftAcceptance)
    case draftPartAccepted(DraftPartAcceptance)
    case finalizeDraftAccepted(FinalizeDraftAcceptance)
    /// §6.1/§6.5's disposition `1`: a same-intent replay of a retained terminal result.
    case alreadyTerminal(ResultEnvelope)
    case operationResult(ResultEnvelope)
    case queryOperationState(QueryOperationState)
    case catalogPage(CatalogPage)
    case draftPage(DraftPage)
    case weatherContext(WeatherRequestContext)
    case deviceStatus(DeviceStatus)
    case configBlock(DeviceConfigBlock)
    case clockStatus(ClockStatus)
    case bondForgotten
    case echoResponse([UInt8])
    case newStoreId(StoreId)

    /// An error response: §12's ErrorBody, or §11's replayed terminal body.
    case error(ErrorBody)
}

/// One complete §2 control frame: the 16-byte header plus its bounded opcode-specific payload.
public struct ControlFrame: Hashable, Sendable {
    public let opcode: Opcode
    public let flags: ControlFlags
    public let requestId: RequestId
    public let body: ControlBody

    public init(opcode: Opcode, flags: ControlFlags, requestId: RequestId, body: ControlBody) {
        self.opcode = opcode
        self.flags = flags
        self.requestId = requestId
        self.body = body
    }

    public var isResponse: Bool { flags.contains(.response) }
    public var isError: Bool { flags.contains(.error) }
    public var hasMore: Bool { flags.contains(.more) }

    // MARK: decode

    /// Decodes exactly one transport record. Total and bounded: every malformed input yields a
    /// `WireFault`.
    public static func decode(_ record: [UInt8]) throws -> ControlFrame {
        // §2: "`invalidFrame` means that a transport record cannot be established as one complete
        // frame." The order below is the validation precedence of §12 — framing, then version,
        // then descriptor.
        guard record.count >= WireLimits.controlHeaderBytes else {
            throw WireFault.recordLength("control record: \(record.count) bytes")
        }
        var reader = ByteReader(record, subject: "control frame")
        let magic = Array(try reader.take(4))
        guard magic == WireLimits.magic else {
            throw WireFault.magic("control frame: \(magic)")
        }
        let major = try reader.u8()
        guard major == WireLimits.major else {
            throw WireFault.unsupportedMajor("control frame: major \(major)")
        }
        let minor = try reader.u8()
        guard minor == WireLimits.minor else {
            throw WireFault.unsupportedMinor("control frame: minor \(minor)")
        }
        let opcodeRaw = try reader.u16()
        let flagsRaw = try reader.u16()
        let payloadLength = Int(try reader.u16())
        let requestIdRaw = try reader.u32()

        guard payloadLength <= WireLimits.maximumControlPayload else {
            throw WireFault.payloadLength("control frame: payload length \(payloadLength)")
        }
        guard record.count == WireLimits.controlHeaderBytes + payloadLength else {
            throw WireFault.payloadLength(
                "control frame: \(record.count) bytes for a declared \(payloadLength)")
        }
        guard flagsRaw & ~ControlFlags.defined == 0 else {
            throw WireFault.unsupportedFlags("control frame: flags \(flagsRaw)")
        }
        let flags = ControlFlags(rawValue: flagsRaw)
        guard let opcode = Opcode(rawValue: opcodeRaw) else {
            throw WireFault.unsupportedOpcode("control frame: opcode \(opcodeRaw)")
        }
        // §2: "Requests have no flags."
        if !flags.contains(.response), !flags.isEmpty {
            throw WireFault.unsupportedFlags("control frame: a request carries flags \(flagsRaw)")
        }
        if flags.contains(.error), !flags.contains(.response) {
            throw WireFault.invalidCombination("control frame: error without response")
        }
        if flags.contains(.more) {
            guard flags.contains(.response), opcode.isPageable, !flags.contains(.error) else {
                throw WireFault.invalidCombination("control frame: more on an unpageable frame")
            }
        }
        // §2: a zero RequestId is unanswerable; the receiver emits no response and closes that
        // record stream, so this reason is recorded and never transmitted.
        guard let requestId = RequestId(requestIdRaw) else {
            throw WireFault.zeroRequestId("control frame: RequestId 0")
        }

        let payload = Array(try reader.take(payloadLength))
        let body: ControlBody
        if flags.contains(.error) {
            body = .error(try ErrorBody.decode(payload))
        } else if flags.contains(.response) {
            body = try decodeResponse(opcode: opcode, payload: payload, more: flags.contains(.more))
        } else {
            body = try decodeRequest(opcode: opcode, payload: payload)
        }
        return ControlFrame(opcode: opcode, flags: flags, requestId: requestId, body: body)
    }

    private static func decodeRequest(opcode: Opcode, payload: [UInt8]) throws -> ControlBody {
        switch opcode {
        case .hello: return .hello(try Hello.decode(payload))
        case .startUpload: return .startUpload(try StartUploadRequest.decode(payload))
        case .checkpointUpload:
            return .checkpointUpload(try CheckpointUploadRequest.decode(payload))
        case .finishUpload:
            try requireExactPayload(payload.count, 4, "FinishUpload")
            var reader = ByteReader(payload, subject: "FinishUpload")
            guard let session = SessionId(try reader.u32()) else {
                throw WireFault.unknownEnum("FinishUpload: zero SessionId")
            }
            return .finishUpload(session)
        case .startDownload: return .startDownload(try StartDownloadRequest.decode(payload))
        case .finishDownload: return .finishDownload(try FinishDownloadRequest.decode(payload))
        case .abortSession: return .abortSession(try AbortSessionRequest.decode(payload))
        case .beginDraft: return .beginDraft(try BeginDraftRequest.decode(payload))
        case .startDraftPart: return .startDraftPart(try StartDraftPartRequest.decode(payload))
        case .finalizeDraft:
            try requireExactPayload(payload.count, 16, "FinalizeDraft")
            return .finalizeDraft(OperationId(unchecked: payload))
        case .queryOperation:
            try requireExactPayload(payload.count, 16, "QueryOperation")
            return .queryOperation(OperationId(unchecked: payload))
        case .queryCatalog: return .queryCatalog(try QueryCatalogRequest.decode(payload))
        case .queryDraft: return .queryDraft(try QueryDraftRequest.decode(payload))
        case .queryWeatherRequest:
            try requireExactPayload(payload.count, 0, "QueryWeatherRequest")
            return .queryWeatherRequest
        case .deleteObject: return .deleteObject(try DeleteObjectRequest.decode(payload))
        case .setMetadata: return .setMetadata(try SetMetadataRequest.decode(payload))
        case .abortOperation: return .abortOperation(try AbortOperationRequest.decode(payload))
        case .installUpdate:
            return .installUpdate(
                try KindedCommandRequest.decode(
                    payload, impliedKind: .updatePackage, subject: "InstallUpdate"))
        case .acknowledgeRideImported:
            return .acknowledgeRideImported(
                try KindedCommandRequest.decode(
                    payload, impliedKind: .ride, subject: "AcknowledgeRideImported"))
        case .getDeviceStatus:
            try requireExactPayload(payload.count, 0, "GetDeviceStatus")
            return .getDeviceStatus
        case .getConfig:
            try requireExactPayload(payload.count, 0, "GetConfig")
            return .getConfig
        case .setConfig: return .setConfig(try DeviceConfigBlock.decode(payload))
        case .setClock: return .setClock(try SetClockRequest.decode(payload))
        case .forgetBond: return .forgetBond(try ForgetBondRequest.decode(payload))
        case .echo:
            // §16: Echo's payload has no internal structure; its maximum is the bound every control
            // frame already has.
            return .echoRequest(payload)
        case .resetStore:
            try requireExactPayload(payload.count, 16, "ResetStore")
            return .resetStore(StoreId(unchecked: payload))
        }
    }

    private static func decodeResponse(opcode: Opcode, payload: [UInt8], more: Bool) throws
        -> ControlBody
    {
        switch opcode {
        case .hello: return .capabilities(try CapabilitiesPage.decode(payload))
        case .startUpload:
            return try decodeDisposition(payload, subject: "UploadAccepted") { reader in
                .uploadAccepted(try UploadAcceptance.decodeAccepted(&reader))
            }
        case .checkpointUpload:
            return .checkpointAccepted(try CheckpointUploadResponse.decode(payload))
        case .finishUpload, .deleteObject, .setMetadata, .abortOperation, .installUpdate,
            .acknowledgeRideImported:
            var reader = ByteReader(payload, subject: "\(opcode) response")
            return .operationResult(try ResultEnvelope.decode(&reader))
        case .startDownload: return .downloadAccepted(try DownloadAcceptance.decode(payload))
        case .finishDownload:
            try requireExactPayload(payload.count, 0, "FinishDownload response")
            return .downloadReleased
        case .abortSession:
            try requireExactPayload(payload.count, 1, "AbortSession response")
            guard let outcome = AbortSessionOutcome(rawValue: payload[0]) else {
                throw WireFault.unknownEnum("AbortSession response: outcome \(payload[0])")
            }
            return .abortSessionOutcome(outcome)
        case .beginDraft:
            return try decodeDisposition(payload, subject: "BeginDraftAccepted") { reader in
                // §6.5: disposition `0` is a four-byte disposition/reserved prefix plus 28 bytes.
                try reader.reserved(3, "BeginDraftAccepted reserved")
                return .beginDraftAccepted(try BeginDraftAcceptance.decode(&reader))
            }
        case .startDraftPart:
            return try decodeDisposition(payload, subject: "DraftPartAccepted") { reader in
                .draftPartAccepted(try DraftPartAcceptance.decodeAccepted(&reader))
            }
        case .finalizeDraft:
            return try decodeDisposition(payload, subject: "FinalizeDraft acceptance") { reader in
                .finalizeDraftAccepted(try FinalizeDraftAcceptance.decodeAccepted(&reader))
            }
        case .queryOperation:
            return .queryOperationState(try QueryOperationState.decode(payload))
        case .queryCatalog: return .catalogPage(try CatalogPage.decode(payload, more: more))
        case .queryDraft: return .draftPage(try DraftPage.decode(payload, more: more))
        case .queryWeatherRequest:
            return .weatherContext(try WeatherRequestContext.decode(payload))
        case .getDeviceStatus: return .deviceStatus(try DeviceStatus.decode(payload))
        case .getConfig, .setConfig: return .configBlock(try DeviceConfigBlock.decode(payload))
        case .setClock: return .clockStatus(try ClockStatus.decode(payload))
        case .forgetBond:
            try requireExactPayload(payload.count, 0, "ForgetBond response")
            return .bondForgotten
        case .echo: return .echoResponse(payload)
        case .resetStore:
            try requireExactPayload(payload.count, 16, "ResetStore response")
            return .newStoreId(StoreId(unchecked: payload))
        }
    }

    /// The shared disposition prologue of §6.1 and §6.5: byte `0` selects between the typed
    /// acceptance and a replay of the retained terminal ResultEnvelope.
    private static func decodeDisposition(
        _ payload: [UInt8], subject: String,
        accepted: (inout ByteReader) throws -> ControlBody
    ) throws -> ControlBody {
        guard !payload.isEmpty else { throw WireFault.truncated("\(subject): empty payload") }
        var reader = ByteReader(payload, subject: subject)
        let raw = try reader.u8()
        guard let disposition = AcceptanceDisposition(rawValue: raw) else {
            throw WireFault.unknownEnum("\(subject): disposition \(raw)")
        }
        switch disposition {
        case .accepted:
            let body = try accepted(&reader)
            try reader.requireExhausted("the acceptance")
            return body
        case .alreadyTerminal:
            try reader.reserved(3, "\(subject): disposition prefix")
            return .alreadyTerminal(try ResultEnvelope.decode(&reader))
        }
    }

    // MARK: encode

    /// Re-encodes the frame byte-exactly, including the derived payload length.
    ///
    /// Fallible by design. §1 and §2 make an over-long frame *unsendable* — "The client MUST NOT
    /// truncate, split, or drop a field to make it fit" — so an encoder that silently produced one
    /// would hand the transport a record the peer must reject as `invalidFrame`. The bound checked
    /// here is the protocol's hard maximum; enforcing the *negotiated* limit is the transport
    /// adapter's seam, because only the adapter knows what Hello agreed and what the link can carry
    /// (§14.0's effective stream limit is not even fixed until CoC establishment).
    public func encoded() throws -> [UInt8] {
        let payload = try encodedPayload()
        try requireAtMost(
            payload.count, WireLimits.maximumControlPayload, "control frame: payload")
        var writer = ByteWriter()
        writer.raw(WireLimits.magic)
        writer.u8(WireLimits.major)
        writer.u8(WireLimits.minor)
        writer.u16(opcode.rawValue)
        writer.u16(flags.rawValue)
        writer.u16(try narrowU16(payload.count, "control frame: payload length"))
        writer.u32(requestId.rawValue)
        writer.raw(payload)
        return writer.bytes
    }

    func encodedPayload() throws -> [UInt8] {
        var writer = ByteWriter()
        switch body {
        case .hello(let value): writer.raw(try value.encoded())
        case .startUpload(let value): writer.raw(try value.encoded())
        case .checkpointUpload(let value): writer.raw(try value.encoded())
        case .finishUpload(let session): writer.u32(session.rawValue)
        case .startDownload(let value): writer.raw(try value.encoded())
        case .finishDownload(let value): writer.raw(try value.encoded())
        case .abortSession(let value): writer.raw(try value.encoded())
        case .beginDraft(let value): writer.raw(try value.encoded())
        case .startDraftPart(let value): writer.raw(try value.encoded())
        case .finalizeDraft(let value): writer.raw(value.bytes)
        case .queryOperation(let value): writer.raw(value.bytes)
        case .queryCatalog(let value): writer.raw(try value.encoded())
        case .queryDraft(let value): writer.raw(try value.encoded())
        case .queryWeatherRequest, .getDeviceStatus, .getConfig, .downloadReleased, .bondForgotten:
            break
        case .deleteObject(let value): writer.raw(try value.encoded())
        case .setMetadata(let value): writer.raw(try value.encoded())
        case .abortOperation(let value): writer.raw(try value.encoded())
        case .installUpdate(let value): writer.raw(try value.encoded())
        case .acknowledgeRideImported(let value): writer.raw(try value.encoded())
        case .setConfig(let value): writer.raw(try value.encoded())
        case .setClock(let value): writer.raw(try value.encoded())
        case .forgetBond(let value): writer.raw(try value.encoded())
        case .echoRequest(let bytes), .echoResponse(let bytes): writer.raw(bytes)
        case .resetStore(let value), .newStoreId(let value): writer.raw(value.bytes)
        case .capabilities(let value): writer.raw(try value.encoded())
        case .uploadAccepted(let value):
            writer.u8(AcceptanceDisposition.accepted.rawValue)
            value.encodeAccepted(into: &writer)
        case .checkpointAccepted(let value): writer.raw(try value.encoded())
        case .downloadAccepted(let value): writer.raw(try value.encoded())
        case .abortSessionOutcome(let outcome): writer.u8(outcome.rawValue)
        case .beginDraftAccepted(let value):
            writer.u8(AcceptanceDisposition.accepted.rawValue)
            writer.zeros(3)
            value.encode(into: &writer)
        case .draftPartAccepted(let value):
            writer.u8(AcceptanceDisposition.accepted.rawValue)
            value.encodeAccepted(into: &writer)
        case .finalizeDraftAccepted(let value):
            writer.u8(AcceptanceDisposition.accepted.rawValue)
            value.encodeAccepted(into: &writer)
        case .alreadyTerminal(let envelope):
            writer.u8(AcceptanceDisposition.alreadyTerminal.rawValue)
            writer.zeros(3)
            envelope.encode(into: &writer)
        case .operationResult(let envelope): envelope.encode(into: &writer)
        case .queryOperationState(let state): writer.raw(try state.encoded())
        case .catalogPage(let page): writer.raw(try page.encoded())
        case .draftPage(let page): writer.raw(try page.encoded())
        case .weatherContext(let context): writer.raw(try context.encoded())
        case .deviceStatus(let status): writer.raw(try status.encoded())
        case .configBlock(let block): writer.raw(try block.encoded())
        case .clockStatus(let status): writer.raw(try status.encoded())
        case .error(let body): writer.raw(try body.encoded())
        }
        return writer.bytes
    }
}
