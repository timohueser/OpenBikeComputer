import Foundation
import OBCProtocolV3

/// One decoded field of a control fixture's `body`: a JSON number for anything at most 32 bits wide,
/// a string for everything else (`Device_Object_Vectors_v2.md` §1).
enum BodyValue: Equatable, CustomStringConvertible {
    case number(Int64)
    case text(String)

    var description: String {
        switch self {
        case .number(let value): return String(value)
        case .text(let value): return "\"\(value)\""
        }
    }
}

/// A control fixture's semantic body: field path to decoded value, built here out of the values the
/// Swift decoder assigned rather than re-read off the bytes.
///
/// `Device_Object_Vectors_v2.md` §1 requires a control fixture to carry "header fields, semantic
/// body, and exact frame hex". Frame hex alone is a *byte* pin: two codecs can agree on every byte
/// and still disagree about which field a byte belongs to, and a codec that transposes two adjacent
/// same-width fields round-trips perfectly. Comparing this map against the fixture's is what makes
/// the suite check the *meaning* this decoder assigned, not only the bytes it gave back.
///
/// The encoding rules, shared by the three language suites:
///
/// - one flat map, keys are field paths — `metadata.field[0].tag`, `entries[1].revision` — so no
///   shared schema is needed to build the same map in any language;
/// - values are numbers only for fields of at most 32 bits and canonical decimal *strings* for every
///   `u64`/`i64`, exactly as §1 requires of the rest of the fixture;
/// - opaque byte fields — identities, diagnostic text, metadata field values, the device name, the
///   Echo payload — are lower-case hex;
/// - enumerated fields carry their **wire number**, never a name, because a name is this module's
///   vocabulary rather than the contract's;
/// - reserved fields never appear: a decoder proves them zero and then has nothing to report.
struct SemanticBody {
    private(set) var fields: [(key: String, value: BodyValue)] = []

    /// A field of at most 32 bits.
    mutating func num(_ key: String, _ value: some BinaryInteger) {
        fields.append((key, .number(Int64(value))))
    }

    /// A `u64` as its canonical decimal string.
    mutating func u64(_ key: String, _ value: UInt64) {
        fields.append((key, .text(String(value))))
    }

    /// An `i64` as its canonical decimal string.
    mutating func i64(_ key: String, _ value: Int64) {
        fields.append((key, .text(String(value))))
    }

    /// An opaque byte field as lower-case hex.
    mutating func hex(_ key: String, _ value: [UInt8]) {
        fields.append((key, .text(value.hexString)))
    }

    /// A boolean as the `0`/`1` its byte carries.
    mutating func flag(_ key: String, _ value: Bool) {
        num(key, value ? 1 : 0)
    }

    /// Splices another body in under a prefix.
    mutating func nest(_ prefix: String, _ other: SemanticBody) {
        for field in other.fields { fields.append((prefix + field.key, field.value)) }
    }

    /// The map keyed by path, refusing a duplicate path rather than letting one silently win — two
    /// fields at one path would make the comparison below check less than it appears to.
    func keyed() throws -> [String: BodyValue] {
        var out: [String: BodyValue] = [:]
        for field in fields {
            guard out.updateValue(field.value, forKey: field.key) == nil else {
                throw VectorError("semantic body emits \(field.key) twice")
            }
        }
        return out
    }
}

/// Builds the semantic body of one decoded control frame.
///
/// This walks the *decoded* Swift types, so a field the decoder placed at the wrong offset, gave the
/// wrong width, or silently dropped shows up as a body mismatch even when the frame re-encodes
/// byte-exactly.
enum ControlBodySemantics {
    static func body(of frame: ControlFrame) -> SemanticBody {
        var body = SemanticBody()
        switch frame.body {
        // §12's ErrorBody is the payload of every `response|error` frame, whatever its opcode.
        case .error(let error):
            body.nest("", errorBody(error))

        // MARK: §5 Hello / Capabilities

        case .hello(let hello):
            body.num("minimumMajor", hello.minimumWireMajor)
            body.num("maximumMajor", hello.maximumWireMajor)
            body.num("clientMaxControlFrame", hello.clientMaximumControlFrame)
            body.num("clientMaxStreamFrame", hello.clientMaximumStreamFrame)
            body.num("clientFeatureFlags", hello.clientFeatureFlags)
            body.num("pageKind", hello.pageKind.rawValue)
            body.num("pageIndex", hello.pageIndex)
        case .capabilities(let page):
            body.nest("", capabilities(page))

        // MARK: §6 upload, checkpoint, finish

        case .startUpload(let request):
            body.hex("operationId", request.operationId.bytes)
            body.num("objectKind", request.objectKind.rawValue)
            body.num("targetMode", request.targetMode.rawValue)
            body.num("resume", request.resume.rawValue)
            body.u64("logicalObjectId", request.logicalObjectId.rawValue)
            body.u64("expectedRevision", request.expectedRevision.rawValue)
            body.u64("declaredLength", request.declaredLength)
            body.num("expectedCrc32", request.expectedCRC32)
            body.nest("metadata.", metadata(request.metadata))
        case .uploadAccepted(let accepted):
            body.num("disposition", 0)
            body.num("targetMode", accepted.targetMode.rawValue)
            body.num("flags", accepted.flags.rawValue)
            body.hex("operationId", accepted.operationId.bytes)
            body.num("sessionId", accepted.sessionId.rawValue)
            body.u64("logicalObjectId", accepted.logicalObjectId.rawValue)
            body.u64("admissionRevision", accepted.repositoryRevisionAtAdmission.rawValue)
            body.u64("durableNextOffset", accepted.durableNextOffset)
            body.num("checkpointGranule", accepted.checkpointGranule)
            body.num("maxStreamPayload", accepted.maximumStreamPayload)
            body.num("finalizedPrefixCrc32", accepted.finalizedPrefixCRC32)
        // §6.1/§6.5's disposition `1`, shared by all four `Start*`/`Finalize` acceptances.
        case .alreadyTerminal(let envelope):
            body.num("disposition", 1)
            body.nest("result.", resultEnvelope(envelope))
        case .checkpointUpload(let request):
            body.num("sessionId", request.sessionId.rawValue)
            body.u64("receivedNextOffset", request.receivedNextOffset)
        case .checkpointAccepted(let response):
            body.num("sessionId", response.sessionId.rawValue)
            body.u64("durableNextOffset", response.durableNextOffset)
            body.num("finalizedPrefixCrc32", response.finalizedPrefixCRC32)
            body.num("checkpointSequence", response.checkpointSequence)
        case .finishUpload(let session):
            body.num("sessionId", session.rawValue)
        case .operationResult(let envelope):
            body.nest("", resultEnvelope(envelope))

        // MARK: §7 download

        case .startDownload(let request):
            body.num("objectKind", request.objectKind.rawValue)
            // The start-offset bit is the only flag a v3.0 StartDownload carries; the decoder keeps
            // it as a Bool, so the wire value is reconstructed from the one bit it can set.
            body.num(
                "flags",
                request.requestsStartOffset ? StartDownloadRequest.Flags.startOffset.rawValue : 0)
            body.u64("logicalObjectId", request.logicalObjectId.rawValue)
            body.u64("startOffset", request.startOffset)
        case .downloadAccepted(let accepted):
            body.hex("storeId", accepted.storeId.bytes)
            body.num("sessionId", accepted.sessionId.rawValue)
            body.u64("logicalObjectId", accepted.logicalObjectId.rawValue)
            body.u64("pinnedRevision", accepted.pinnedRevision.rawValue)
            body.u64("totalLength", accepted.totalLength)
            body.num("wholeSourceCrc32", accepted.wholeSourceCRC32)
            body.u64("acceptedStartOffset", accepted.acceptedStartOffset)
            body.num("maxStreamPayload", accepted.maximumStreamPayload)
        case .finishDownload(let request):
            body.num("sessionId", request.sessionId.rawValue)
            body.u64("receivedLength", request.receivedWholeSourceLength)
            body.num("wholeSourceCrc32", request.wholeSourceCRC32)

        // MARK: §6.4 abort

        case .abortSession(let request):
            body.num("sessionId", request.sessionId.rawValue)
            body.num("reason", request.reason.rawValue)
        case .abortSessionOutcome(let outcome):
            body.num("outcome", outcome.rawValue)
        case .abortOperation(let request):
            body.hex("operationId", request.abortCommandOperationId.bytes)
            body.hex("targetOperationId", request.targetOperationId.bytes)
            body.num("reason", request.reason.rawValue)

        // MARK: §6.5 drafts

        case .beginDraft(let request):
            body.hex("parentOperationId", request.parentOperationId.bytes)
            body.num("objectKind", request.finalObjectKind.rawValue)
            body.num("targetMode", request.targetMode.rawValue)
            body.u64("logicalObjectId", request.logicalObjectId.rawValue)
            body.u64("expectedRevision", request.expectedRevision.rawValue)
            body.u64("declaredManifestLength", request.declaredManifestLength)
            body.num("declaredManifestCrc32", request.declaredManifestCRC32)
            body.num("expectedPartCount", request.expectedPartCount)
        case .beginDraftAccepted(let accepted):
            body.num("disposition", 0)
            body.hex("parentOperationId", accepted.parentOperationId.bytes)
            body.u64("draftRevision", accepted.draftRevision.rawValue)
            body.num("expectedPartCount", accepted.expectedParts)
            body.num("state", accepted.state.rawValue)
        case .startDraftPart(let request):
            body.hex("childOperationId", request.childOperationId.bytes)
            body.hex("parentOperationId", request.parentOperationId.bytes)
            body.num("partKind", request.draftPartKind.rawValue)
            body.u64("partKey", request.partKey.rawValue)
            body.u64("declaredLength", request.declaredLength)
            body.num("expectedCrc32", request.expectedCRC32)
            body.num("resume", request.resume.rawValue)
        case .draftPartAccepted(let accepted):
            body.num("disposition", 0)
            body.num("flags", accepted.flags.rawValue)
            body.hex("childOperationId", accepted.childOperationId.bytes)
            body.hex("parentOperationId", accepted.parentOperationId.bytes)
            body.num("sessionId", accepted.sessionId.rawValue)
            body.num("partKind", accepted.draftPartKind.rawValue)
            body.u64("partKey", accepted.partKey.rawValue)
            body.u64("durableNextOffset", accepted.durableNextOffset)
            body.num("checkpointGranule", accepted.checkpointGranule)
            body.num("maxStreamPayload", accepted.maximumStreamPayload)
            body.num("finalizedPrefixCrc32", accepted.finalizedPrefixCRC32)
        case .finalizeDraft(let parent):
            body.hex("parentOperationId", parent.bytes)
        case .finalizeDraftAccepted(let accepted):
            body.num("disposition", 0)
            body.num("flags", accepted.flags.rawValue)
            body.hex("parentOperationId", accepted.parentOperationId.bytes)
            body.num("sessionId", accepted.sessionId.rawValue)
            body.u64("logicalObjectId", accepted.logicalObjectId.rawValue)
            body.u64("admissionRevision", accepted.repositoryRevisionAtAdmission.rawValue)
            body.u64("durableManifestOffset", accepted.durableManifestOffset)
            body.num("checkpointGranule", accepted.checkpointGranule)
            body.num("maxStreamPayload", accepted.maximumStreamPayload)
            body.num("finalizedPrefixCrc32", accepted.finalizedPrefixCRC32)

        // MARK: §8 queries

        case .queryOperation(let operationId):
            body.hex("operationId", operationId.bytes)
        case .queryOperationState(let state):
            switch state {
            case .unknown:
                body.num("state", 0)
            case .inProgress(let progress):
                body.num("state", 1)
                body.nest("progress.", progressBody(progress))
            case .committed(let envelope):
                body.num("state", 2)
                body.nest("result.", resultEnvelope(envelope))
            case .aborted(let error):
                body.num("state", 3)
                body.nest("error.", errorBody(error))
            }
        case .queryCatalog(let request):
            body.num("objectKind", request.objectKind.rawValue)
            body.num("flags", request.flags.rawValue)
            body.u64("expectedRevision", request.expectedRevision.rawValue)
            body.nest("cursor.", cursor(request.cursor))
        case .catalogPage(let page):
            body.hex("storeId", page.storeId.bytes)
            body.num("objectKind", page.objectKind.rawValue)
            body.num("entryCount", page.entries.count)
            body.u64("revision", page.repositoryRevision.rawValue)
            body.nest("nextCursor.", cursor(page.nextCursor))
            for (index, entry) in page.entries.enumerated() {
                var inner = SemanticBody()
                inner.u64("logicalObjectId", entry.logicalObjectId.rawValue)
                inner.u64("revision", entry.revision.rawValue)
                inner.u64("length", entry.length)
                inner.num("crc32", entry.crc32)
                inner.nest("metadata.", metadata(entry.metadata))
                body.nest("entries[\(index)].", inner)
            }
        case .queryDraft(let request):
            body.hex("parentOperationId", request.parentOperationId.bytes)
            body.num("flags", request.flags.rawValue)
            body.num("requestedLimit", request.requestedLimit)
            body.u64("expectedRevision", request.expectedDraftRevision.rawValue)
            body.nest("cursor.", cursor(request.cursor))
        case .draftPage(let page):
            body.hex("parentOperationId", page.parentOperationId.bytes)
            body.u64("draftRevision", page.draftRevision.rawValue)
            body.nest("nextCursor.", cursor(page.nextCursor))
            body.num("entryCount", page.entries.count)
            body.num("flags", page.flags.rawValue)
            for (index, entry) in page.entries.enumerated() {
                var inner = SemanticBody()
                inner.hex("childOperationId", entry.childOperationId.bytes)
                inner.hex("draftPartRef", entry.draftPartRef.bytes)
                inner.num("partKind", entry.draftPartKind.rawValue)
                inner.u64("partKey", entry.partKey.rawValue)
                inner.num("state", entry.state.rawValue)
                inner.u64("durableOffset", entry.durableOffset)
                inner.u64("declaredLength", entry.declaredLength)
                inner.num("crc32", entry.crc32)
                body.nest("entries[\(index)].", inner)
            }
        case .queryWeatherRequest:
            break
        case .weatherContext(let context):
            body.hex("storeId", context.storeId.bytes)
            body.u64("currentWeatherRequestId", context.currentWeatherRequestId.rawValue)
            body.u64("contextRevision", context.requestContextRevision)
            // §8.4's flags word carries one defined bit in v3.0: head-present.
            body.num("flags", context.headPresent ? 1 : 0)
            body.u64("weatherLogicalObjectId", context.weatherLogicalObjectId.rawValue)
            body.u64("repositoryRevision", context.weatherRepositoryRevision.rawValue)
            body.u64("headWeatherRequestId", context.headWeatherRequestId.rawValue)
            body.num("centreLatitudeE7", context.centreLatitudeE7)
            body.num("centreLongitudeE7", context.centreLongitudeE7)
            body.num("radiusMetres", context.requiredRadiusMetres)
            body.i64("earliestIssuedUtc", context.earliestIssuedUTC)
            body.i64("requiredValidUntilUtc", context.requiredValidUntilUTC)
            body.num("state", context.state.rawValue)

        // MARK: §9 direct mutations

        case .deleteObject(let request):
            body.nest("", mutationTarget(request.target))
        case .setMetadata(let request):
            body.nest("", mutationTarget(request.target))
            body.nest("patch.", metadata(request.patch))
        case .installUpdate(let request), .acknowledgeRideImported(let request):
            body.hex("operationId", request.operationId.bytes)
            body.u64("logicalObjectId", request.logicalObjectId.rawValue)
            body.u64("expectedRevision", request.expectedRevision.rawValue)

        // MARK: §16 device control

        case .getDeviceStatus, .getConfig, .downloadReleased, .bondForgotten:
            break
        case .deviceStatus(let status):
            body.num("firmwareMajor", status.firmwareMajor)
            body.num("firmwareMinor", status.firmwareMinor)
            body.num("firmwarePatch", status.firmwarePatch)
            body.num("hardwareRevision", status.hardwareRevision)
            body.hex("deviceSerial", status.serial.bytes)
            body.num("bootCount", status.bootCount)
            body.u64("uptimeSeconds", status.uptimeSeconds)
            body.num("stackHighWater", status.worstStackHighWaterBytes)
            body.num("statusFlags", status.flags.rawValue)
            body.num("mountClass", status.mountClass.rawValue)
            body.num("firmwareBuild", status.firmwareBuildNumber)
            body.hex("storeId", status.storeId.bytes)
        case .setConfig(let block), .configBlock(let block):
            body.num("codecVersion", block.codecVersion)
            body.num("blockLength", DeviceConfigBlock.payloadBytes)
            body.num("nameLength", block.nameBytes.count)
            body.num("unitFlags", block.unitFlags.rawValue)
            body.num("weatherRefresh", block.weatherRefresh.rawValue)
            body.hex("name", block.nameBytes)
        case .setClock(let request):
            body.i64("epochSeconds", request.epochSeconds)
            body.num("source", request.source.rawValue)
        case .clockStatus(let status):
            body.i64("epochSeconds", status.epochSeconds)
            body.num("source", status.rawSource)
            body.num("state", status.state.rawValue)
        case .forgetBond(let request):
            body.num("scope", request.scope.rawValue)
        case .echoRequest(let payload), .echoResponse(let payload):
            body.hex("payload", payload)
        case .resetStore(let echo):
            body.hex("echoStoreId", echo.bytes)
        case .newStoreId(let storeId):
            body.hex("newStoreId", storeId.bytes)
        }
        return body
    }

    // MARK: shared substructures

    /// §5's common prefix, then whichever page body its page kind selects.
    private static func capabilities(_ page: CapabilitiesPage) -> SemanticBody {
        var body = SemanticBody()
        body.num("selectedMajor", page.selectedWireMajor)
        body.num("storageFormatVersion", page.storageFormatVersion)
        body.num("statusFlags", page.statusFlags.rawValue)
        body.hex("storeId", page.storeId.bytes)
        body.num("negotiatedControlFrame", page.negotiatedMaximumControlFrame)
        body.num("negotiatedStreamFrame", page.negotiatedMaximumStreamFrame)
        body.num("checkpointGranule", page.checkpointGranule)
        body.num("retainedResultCapacity", page.retainedResultCapacity)
        body.num("metadataEnvelopeLimit", page.metadataEnvelopeLimit)
        body.num("catalogMetadataLimit", page.catalogMetadataLimit)
        body.num("protocolMinimumControlFrame", page.protocolMinimumControlFrame)
        body.num("protocolMinimumStreamFrame", page.protocolMinimumStreamFrame)
        body.num("linkKind", page.linkKind.rawValue)
        body.flag("authenticated", page.authenticated)
        body.num("capabilityRevision", page.capabilityRevision)
        body.num("commandFlags", page.commandFlags.rawValue)
        body.num("totalSubjectCount", page.totalSubjectCount)
        body.num("pageKind", page.returnedPageKind.rawValue)
        body.num("pageIndex", page.returnedPageIndex)
        body.num("returnedSubjectCount", page.subjects.count)
        body.num("totalPages", page.totalPagesOfThisKind)
        body.num("deviceWireMinor", page.deviceWireMinor)
        if let limits = page.resourceLimits {
            var inner = SemanticBody()
            inner.num("codecVersion", limits.codecVersion)
            inner.num("blockLength", ResourceLimits.blockBytes)
            inner.num("logicalCatalogHeads", limits.logicalCatalogHeads)
            inner.num("normalClaims", limits.normalActiveClaimedOperations)
            inner.num("uploadWorkSlots", limits.resumableWorkSlots)
            inner.num("draftParents", limits.activeDraftParents)
            inner.num("draftParts", limits.draftPartsOfActiveParent)
            inner.num("manifestChildren", limits.childrenPerManifest)
            inner.num("mountedFiles", limits.mountedMapDataFiles)
            inner.num("readerLeases", limits.liveReaderLeases)
            inner.num("retainedGenerations", limits.retainedPreviousGenerations)
            inner.num("retainedResults", limits.retainedTerminalResults)
            inner.num("inactiveWorkHorizon", limits.inactiveWorkHorizon)
            inner.u64("maxGenerationLength", limits.maximumSingleGenerationLength)
            inner.u64("availableReservationBytes", limits.currentlyAvailableReservationBytes)
            inner.num("routeHeads", limits.routeCatalogHeads)
            inner.num("tripHeads", limits.tripCatalogHeads)
            inner.num("rideHeads", limits.rideCatalogHeads)
            inner.num("weatherHeads", limits.weatherCatalogHeads)
            inner.num("volumeManifestHeads", limits.volumeManifestCatalogHeads)
            inner.num("updatePackageHeads", limits.updatePackageCatalogHeads)
            inner.num("heavyStreamSessions", limits.attachedHeavyStreamSessions)
            inner.num("maintenanceClaims", limits.reservedMaintenanceClaims)
            inner.num("rideSlots", limits.activeOrRecoverableRideSlots)
            body.nest("resourceLimits.", inner)
        }
        for (index, subject) in page.subjects.enumerated() {
            var inner = SemanticBody()
            inner.num("namespace", subject.namespace.rawValue)
            inner.num("kindCode", subject.kindCode)
            inner.num("operationFlags", subject.operationFlags.rawValue)
            inner.num("policyFlags", subject.policyFlags.rawValue)
            inner.num("putSchemaVersion", subject.putSchemaVersion)
            inner.num("patchSchemaVersion", subject.patchSchemaVersion)
            inner.num("catalogSchemaVersion", subject.catalogSchemaVersion)
            inner.u64("maxLength", subject.maximumLength)
            body.nest("subjects[\(index)].", inner)
        }
        return body
    }

    /// §2.2's metadata envelope: the header, then one row per field.
    private static func metadata(_ envelope: MetadataEnvelope) -> SemanticBody {
        var body = SemanticBody()
        body.num("schemaId", envelope.schemaId)
        body.num("schemaVersion", envelope.schemaVersion)
        body.num("encodedFieldBytes", envelope.encodedFieldBytes)
        body.num("fieldCount", envelope.fields.count)
        for (index, field) in envelope.fields.enumerated() {
            body.num("field[\(index)].tag", field.tag)
            body.hex("field[\(index)].value", field.value)
        }
        return body
    }

    /// §8.2's sixteen-byte cursor, which both paged queries carry.
    private static func cursor(_ cursor: CatalogCursor) -> SemanticBody {
        var body = SemanticBody()
        body.u64("revision", cursor.revision.rawValue)
        body.num("nextEntryIndex", cursor.nextEntryIndex)
        body.num("kindCode", cursor.objectKindCode)
        body.num("crc32", cursor.checksum)
        return body
    }

    /// §12's ErrorBody. Category, guidance and owner are reported as their wire numbers even when
    /// this codec has no case for them: an unknown nonzero category is kept, not rejected.
    private static func errorBody(_ error: ErrorBody) -> SemanticBody {
        var body = SemanticBody()
        body.num("category", error.rawCategory)
        body.num("detailNamespace", error.namespace)
        body.num("detail", error.detail)
        body.num("guidance", error.rawGuidance)
        body.num("owner", error.rawOwner)
        body.num("presence", error.presence.rawValue)
        body.num("retryAfterMs", error.retryAfterMilliseconds)
        body.u64("expectedOffset", error.expectedOffset)
        body.u64("currentRevision", error.currentRevision)
        body.u64("requiredBytes", error.requiredBytes)
        body.u64("availableBytes", error.availableBytes)
        body.num("textLength", error.text.count)
        body.hex("text", error.text)
        return body
    }

    /// §10's ResultEnvelope: a type byte and the typed body it introduces.
    private static func resultEnvelope(_ envelope: ResultEnvelope) -> SemanticBody {
        var body = SemanticBody()
        body.num("resultType", envelope.resultType)
        switch envelope {
        case .object(let result):
            body.hex("operationId", result.operationId.bytes)
            body.hex("storeId", result.storeId.bytes)
            body.num("objectKind", result.objectKind.rawValue)
            body.num("outcome", result.outcome.rawValue)
            body.u64("logicalObjectId", result.logicalObjectId.rawValue)
            body.u64("revision", result.newRevision.rawValue)
            body.u64("length", result.length)
            body.num("crc32", result.crc32)
        case .draftPart(let result):
            body.hex("childOperationId", result.childOperationId.bytes)
            body.hex("storeId", result.storeId.bytes)
            body.hex("parentOperationId", result.parentOperationId.bytes)
            body.hex("draftPartRef", result.draftPartRef.bytes)
            body.num("partKind", result.draftPartKind.rawValue)
            body.u64("partKey", result.partKey.rawValue)
            body.u64("length", result.length)
            body.num("crc32", result.crc32)
        case .abort(let result):
            body.hex("operationId", result.abortCommandOperationId.bytes)
            body.hex("storeId", result.storeId.bytes)
            body.hex("targetOperationId", result.targetOperationId.bytes)
            body.num("disposition", result.disposition.rawValue)
        }
        return body
    }

    /// §8.1's 24-byte progress body.
    private static func progressBody(_ progress: OperationProgress) -> SemanticBody {
        var body = SemanticBody()
        body.num("namespace", progress.subjectNamespace.rawValue)
        body.num("phase", progress.phase.rawValue)
        body.num("flags", progress.flags.rawValue)
        body.num("subjectKind", progress.subjectKind)
        body.u64("logicalObjectId", progress.logicalObjectId.rawValue)
        body.u64("durableOffset", progress.durableOffset)
        return body
    }

    /// §9's 36 bytes. The expected-revision flag is mandatory on both direct mutations, so the codec
    /// models it as always set rather than as a field it stores.
    private static func mutationTarget(_ target: DirectMutationTarget) -> SemanticBody {
        var body = SemanticBody()
        body.hex("operationId", target.operationId.bytes)
        body.num("objectKind", target.objectKind.rawValue)
        body.num("flags", 1)
        body.u64("logicalObjectId", target.logicalObjectId.rawValue)
        body.u64("expectedRevision", target.expectedRevision.rawValue)
        return body
    }
}
