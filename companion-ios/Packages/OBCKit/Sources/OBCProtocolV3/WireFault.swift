import Foundation

/// The machine-readable error taxonomy of `specs/Device_Object_Protocol_v3.md` §12. Category `0` is
/// reserved and invalid — it is never a case here.
public enum ErrorCategory: UInt16, Sendable, CaseIterable {
    case incompatibleVersion = 1
    case unsupportedCapability = 2
    case authenticationFailed = 3
    case authorizationFailed = 4
    case busy = 5
    case invalidFrame = 6
    case invalidDescriptor = 7
    case invalidOffset = 8
    case invalidSession = 9
    case objectNotFound = 10
    case revisionConflict = 11
    case insufficientSpace = 12
    case checksumFailure = 13
    case semanticValidation = 14
    case mediaUnavailable = 15
    case mediaIo = 16
    case cancelled = 17
    case linkLost = 18
    case operationIdConflict = 19
    case resourceLimit = 20
    case catalogChanged = 21
    case `internal` = 22

    public var name: String {
        switch self {
        case .incompatibleVersion: return "incompatibleVersion"
        case .unsupportedCapability: return "unsupportedCapability"
        case .authenticationFailed: return "authenticationFailed"
        case .authorizationFailed: return "authorizationFailed"
        case .busy: return "busy"
        case .invalidFrame: return "invalidFrame"
        case .invalidDescriptor: return "invalidDescriptor"
        case .invalidOffset: return "invalidOffset"
        case .invalidSession: return "invalidSession"
        case .objectNotFound: return "objectNotFound"
        case .revisionConflict: return "revisionConflict"
        case .insufficientSpace: return "insufficientSpace"
        case .checksumFailure: return "checksumFailure"
        case .semanticValidation: return "semanticValidation"
        case .mediaUnavailable: return "mediaUnavailable"
        case .mediaIo: return "mediaIo"
        case .cancelled: return "cancelled"
        case .linkLost: return "linkLost"
        case .operationIdConflict: return "operationIdConflict"
        case .resourceLimit: return "resourceLimit"
        case .catalogChanged: return "catalogChanged"
        case .internal: return "internal"
        }
    }

    /// §13's stream fault-body transport set: "exactly these ten categories and no others".
    ///
    /// `resourceLimit` is deliberately excluded — every bounded resource a stream could exhaust is
    /// reserved at admission, so an attached session has no resource-limit condition to report —
    /// and `semanticValidation` is excluded because the body has no namespace field to scope its
    /// detail. Everything else reaches the client through a correlated control response.
    public var isStreamFaultCategory: Bool {
        switch self {
        case .invalidFrame, .invalidDescriptor, .invalidOffset, .invalidSession, .checksumFailure,
            .mediaUnavailable, .mediaIo, .cancelled, .linkLost, .internal:
            return true
        case .incompatibleVersion, .unsupportedCapability, .authenticationFailed,
            .authorizationFailed, .busy, .objectNotFound, .revisionConflict, .insufficientSpace,
            .semanticValidation, .operationIdConflict, .resourceLimit, .catalogChanged:
            return false
        }
    }
}

/// §12's retry guidance enum.
public enum RetryGuidance: UInt8, Sendable, CaseIterable {
    case rejectPermanently = 0
    case retrySameRequest = 1
    case retryAfterSuppliedDelay = 2
    case retryAfterOwnerRelease = 3
    case reconnectThenQueryOperation = 4
    case queryOperationNow = 5
    case resumeAtExpectedOffset = 6
    case refreshCatalogOrDomainState = 7
    case newOperationIdForNewIntent = 8
    case retryOnlyAfterUserAction = 9
}

/// §12's owner byte. Values `1`, `2`, `3` deliberately agree with the link-kind byte of §5, but the
/// two are separate namespaces and this type never converts into `LinkKind`.
public enum ErrorOwner: UInt8, Sendable, CaseIterable {
    case none = 0
    case ble = 1
    case usb = 2
    case test = 3
    case localProducer = 4
    case maintenance = 5
}

/// Namespace-zero detail codes of §12, one nested enum per category. Names match the contract's
/// table verbatim, because a client's log line and a fixture's `detail` string are the same string.
public enum CommonDetail {
    public static func name(category: ErrorCategory, code: UInt16) -> String? {
        if code == 0 { return nil }
        return table[category]?[code]
    }

    public static func code(category: ErrorCategory, name: String) -> UInt16? {
        table[category]?.first { $0.value == name }?.key
    }

    static let table: [ErrorCategory: [UInt16: String]] = [
        .incompatibleVersion: [1: "unsupportedMajor", 2: "unsupportedMinor"],
        .unsupportedCapability: [
            1: "opcode", 2: "logicalKind", 3: "draftPartKind", 4: "feature", 5: "schemaVersion",
            6: "nonCancellableOperation",
        ],
        .authenticationFailed: [
            1: "missingCredential", 2: "invalidCredential", 3: "expiredCredential",
        ],
        .authorizationFailed: [
            1: "principalScope", 2: "operationOwner", 3: "domainRead", 4: "domainWrite",
            5: "installAuthority", 6: "deviceControl",
        ],
        .busy: [
            1: "heavyTransfer", 2: "normalOperationClaims", 3: "uploadWorkSlots", 4: "draftParents",
            5: "draftParts", 6: "readerLeases", 7: "maintenanceCancellationRecoveryClaim",
            8: "maintenance", 9: "rideSlot", 10: "retainedPrevious",
        ],
        .invalidFrame: [
            1: "malformedHeader", 2: "recordLength", 3: "magic", 4: "payloadLength",
            5: "frameBounds", 6: "truncated", 7: "trailingBytes",
        ],
        .invalidDescriptor: [
            1: "reservedBits", 2: "unknownEnum", 3: "invalidCombination", 4: "nestedLength",
            5: "noncanonicalMetadata", 6: "duplicateField", 7: "outOfOrderField",
            8: "unsupportedFlags", 9: "zeroRequestId", 10: "emptyMetadataPatch",
        ],
        .invalidOffset: [1: "unexpectedOffset", 2: "checkpointBoundary"],
        .invalidSession: [
            1: "unknown", 2: "staleConnection", 3: "wrongPrincipal", 4: "wrongLink",
            5: "wrongDirection",
        ],
        .objectNotFound: [
            1: "logicalObject", 2: "requestedRevision", 3: "draftParentUnknown",
            4: "operationTerminal", 5: "resumableWork", 6: "weatherRequestContext",
        ],
        .revisionConflict: [1: "object", 2: "repository", 3: "singleton"],
        .insufficientSpace: [1: "reservationBytes", 2: "catalogCapacity", 3: "retainedPrevious"],
        .checksumFailure: [1: "wholePayload", 2: "durablePrefix", 3: "cursor"],
        // §12: with namespace `0`, semanticValidation carries the device-control plane's one row.
        .semanticValidation: [1: "clockRegression"],
        .mediaUnavailable: [1: "noCard", 2: "unmounted", 3: "recoveryReadOnly"],
        .mediaIo: [1: "read", 2: "write", 3: "synchronize", 4: "uncertainCommit"],
        .cancelled: [
            1: "clientCancelled", 2: "superseded", 3: "userRequested", 4: "workExpired",
        ],
        .linkLost: [1: "control", 2: "stream"],
        .operationIdConflict: [1: "intentDigest"],
        .resourceLimit: [
            1: "minimumControlFrame", 2: "minimumStreamFrame", 3: "objectLength",
            4: "normalOperationClaims", 5: "uploadWorkSlots", 6: "draftParents", 7: "draftParts",
            8: "manifestChildren", 9: "readerLeases", 10: "catalogHeads", 11: "mountedFiles",
            12: "rideSlot",
        ],
        .catalogChanged: [1: "catalogSnapshot", 2: "draftSnapshot", 3: "capabilitySnapshot"],
        .internal: [1: "invariant", 2: "codec", 3: "recoveryReconciliation"],
    ]

    /// The nine rows §12 keeps registered so their numbers stay burned, and which a conforming v3.0
    /// device never emits. A decoder still reads them.
    public static let reservedInV3: Set<Pair> = [
        Pair(.insufficientSpace, 3), Pair(.busy, 5), Pair(.resourceLimit, 6), Pair(.busy, 10),
        Pair(.resourceLimit, 12), Pair(.busy, 8), Pair(.catalogChanged, 3), Pair(.objectNotFound, 2),
        Pair(.objectNotFound, 5),
    ]

    public struct Pair: Hashable, Sendable {
        public let category: ErrorCategory
        public let code: UInt16
        public init(_ category: ErrorCategory, _ code: UInt16) {
            self.category = category
            self.code = code
        }
    }
}

/// The domain-scoped `semanticValidation` details of `Device_Object_Registries_v2.md` §6.
public enum SemanticDetail {
    static let table: [ObjectKind: [UInt16: String]] = [
        .route: [1: "invalidRouteFormat"],
        .trip: [1: "invalidTripFormat", 2: "duplicateRouteReference", 3: "missingTripRoute"],
        .ride: [1: "invalidRideFormat", 2: "alreadyImported"],
        .weather: [
            1: "supersededNotUseful", 2: "coverageMismatch", 3: "staleBundle",
            4: "payloadFactsMismatch", 5: "requestMismatch",
        ],
        .volumeManifest: [
            1: "invalidManifest", 2: "missingDraftPart", 3: "foreignDraftPart",
            4: "duplicateDraftReference", 5: "duplicateDraftPart", 6: "draftNotOpen",
            7: "draftIncomplete",
        ],
        .updatePackage: [
            1: "invalidSignature", 2: "digestMismatch", 3: "wrongTarget", 4: "downgradeDenied",
            5: "packageTooLarge", 6: "unsafePowerState", 7: "unsafeRuntimeState",
            8: "notVerifiedReady",
        ],
    ]

    public static func name(kind: ObjectKind, code: UInt16) -> String? {
        if code == 0 { return nil }
        return table[kind]?[code]
    }
}

/// A typed rejection. Bounded, total decoding means every malformed input produces one of these and
/// never a trap, a `fatalError`, or an out-of-range read.
public struct WireFault: Error, Hashable, Sendable, CustomStringConvertible {
    public let category: ErrorCategory
    /// Category-scoped detail code; `0` means "no narrower fact".
    public let detail: UInt16
    /// Detail namespace: common `0`, or the affected ObjectKind for `semanticValidation`.
    public let namespace: UInt16
    /// Non-normative: what the decoder was looking at. Never transmitted.
    public let context: String

    public init(
        _ category: ErrorCategory, _ detail: UInt16, namespace: UInt16 = 0, context: String = ""
    ) {
        self.category = category
        self.detail = detail
        self.namespace = namespace
        self.context = context
    }

    public var detailName: String? {
        if category == .semanticValidation, namespace != 0, let kind = ObjectKind(rawValue: namespace) {
            return SemanticDetail.name(kind: kind, code: detail)
        }
        return CommonDetail.name(category: category, code: detail)
    }

    public var description: String {
        "\(category.name)/\(detailName ?? String(detail))\(context.isEmpty ? "" : " (\(context))")"
    }

    public static func == (lhs: WireFault, rhs: WireFault) -> Bool {
        lhs.category == rhs.category && lhs.detail == rhs.detail && lhs.namespace == rhs.namespace
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(category)
        hasher.combine(detail)
        hasher.combine(namespace)
    }

    // Shorthands for the details this codec raises, so a call site reads like the contract.
    static func recordLength(_ c: String) -> WireFault { .init(.invalidFrame, 2, context: c) }
    static func magic(_ c: String) -> WireFault { .init(.invalidFrame, 3, context: c) }
    static func payloadLength(_ c: String) -> WireFault { .init(.invalidFrame, 4, context: c) }
    static func frameBounds(_ c: String) -> WireFault { .init(.invalidFrame, 5, context: c) }
    static func truncated(_ c: String) -> WireFault { .init(.invalidFrame, 6, context: c) }
    static func trailingBytes(_ c: String) -> WireFault { .init(.invalidFrame, 7, context: c) }
    static func malformedHeader(_ c: String) -> WireFault { .init(.invalidFrame, 1, context: c) }

    static func reservedBits(_ c: String) -> WireFault { .init(.invalidDescriptor, 1, context: c) }
    static func unknownEnum(_ c: String) -> WireFault { .init(.invalidDescriptor, 2, context: c) }
    static func invalidCombination(_ c: String) -> WireFault {
        .init(.invalidDescriptor, 3, context: c)
    }
    static func nestedLength(_ c: String) -> WireFault { .init(.invalidDescriptor, 4, context: c) }
    static func noncanonicalMetadata(_ c: String) -> WireFault {
        .init(.invalidDescriptor, 5, context: c)
    }
    static func duplicateField(_ c: String) -> WireFault { .init(.invalidDescriptor, 6, context: c) }
    static func outOfOrderField(_ c: String) -> WireFault {
        .init(.invalidDescriptor, 7, context: c)
    }
    static func unsupportedFlags(_ c: String) -> WireFault {
        .init(.invalidDescriptor, 8, context: c)
    }
    static func zeroRequestId(_ c: String) -> WireFault { .init(.invalidDescriptor, 9, context: c) }
    static func emptyMetadataPatch(_ c: String) -> WireFault {
        .init(.invalidDescriptor, 10, context: c)
    }
    /// §8.2/§8.3: a cursor whose CRC-32 does not reproduce under the store that minted it.
    static func cursorChecksum(_ c: String) -> WireFault { .init(.checksumFailure, 3, context: c) }
    static func unsupportedOpcode(_ c: String) -> WireFault {
        .init(.unsupportedCapability, 1, context: c)
    }
    static func unsupportedLogicalKind(_ c: String) -> WireFault {
        .init(.unsupportedCapability, 2, context: c)
    }
    static func unsupportedSchemaVersion(_ c: String) -> WireFault {
        .init(.unsupportedCapability, 5, context: c)
    }
    static func unsupportedMajor(_ c: String) -> WireFault {
        .init(.incompatibleVersion, 1, context: c)
    }
    static func unsupportedMinor(_ c: String) -> WireFault {
        .init(.incompatibleVersion, 2, context: c)
    }
}
