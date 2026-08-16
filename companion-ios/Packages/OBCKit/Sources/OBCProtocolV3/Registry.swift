import Foundation

/// `Device_Object_Registries_v2.md` §1. Value `0` (invalid) and `5` (reserved) are never encoded, so
/// they are not cases.
public enum ObjectKind: UInt16, Sendable, CaseIterable {
    case route = 1
    case trip = 2
    case ride = 3
    case weather = 4
    case volumeManifest = 6
    case updatePackage = 7

    public var name: String {
        switch self {
        case .route: return "route"
        case .trip: return "trip"
        case .ride: return "ride"
        case .weather: return "weather"
        case .volumeManifest: return "volumeManifest"
        case .updatePackage: return "updatePackage"
        }
    }
}

/// `Device_Object_Registries_v2.md` §2. Value `0` (invalid) is never encoded.
public enum DraftPartKind: UInt16, Sendable, CaseIterable {
    case standaloneMapBlob = 1
    case mapShard = 2
    case terrainBlob = 3
    case volumeIndex = 4
}

/// §4's operation registry. Requests and successful responses share an opcode.
public enum Opcode: UInt16, Sendable, CaseIterable {
    case hello = 0x0001
    case startUpload = 0x0100
    case checkpointUpload = 0x0101
    case finishUpload = 0x0102
    case startDownload = 0x0110
    case finishDownload = 0x0111
    case abortSession = 0x0120
    case beginDraft = 0x0130
    case startDraftPart = 0x0131
    case finalizeDraft = 0x0132
    case queryOperation = 0x0200
    case queryCatalog = 0x0201
    case queryDraft = 0x0202
    case queryWeatherRequest = 0x0203
    case deleteObject = 0x0300
    case setMetadata = 0x0301
    case abortOperation = 0x0302
    case installUpdate = 0x0310
    case acknowledgeRideImported = 0x0311
    case getDeviceStatus = 0x0400
    case getConfig = 0x0401
    case setConfig = 0x0402
    case setClock = 0x0403
    case forgetBond = 0x0404
    case echo = 0x0405
    case resetStore = 0x0406

    /// §2: `more` is valid only on a paged Capabilities, QueryCatalog, or QueryDraft response.
    public var isPageable: Bool {
        self == .hello || self == .queryCatalog || self == .queryDraft
    }

    /// §16: the `0x04xx` device-control plane carries no OperationId, claims nothing, and never
    /// touches the catalog.
    public var isDeviceControl: Bool { rawValue & 0xFF00 == 0x0400 }
}

/// The hard limits table of §1, plus the two derived floors §2.2 requires codec tests to recompute.
///
/// **These are the *protocol* limits, and they are the only ones this codec enforces.** §1 defines
/// two different bounds and they belong to two different layers:
///
/// - the **hard maximum** (512-byte control frame, 4,096-byte stream frame, 496-byte control
///   payload) is a property of the contract, identical on every link and knowable without any
///   negotiation — so a decoder rejects a frame above it as `invalidFrame` before allocating, and
///   an encoder refuses to emit one;
/// - the **negotiated limit** — "the smaller supported value advertised by the two peers" — is a
///   property of one connection. It is the **transport adapter's** seam, not this codec's: only the
///   adapter knows what Hello agreed, and on BLE §14.0's effective stream limit is
///   `min(negotiated stream maximum, CoC SDU)` and is not even fixed until CoC establishment,
///   which happens after Hello. A codec that tried to enforce it would need connection state it
///   deliberately does not have.
///
/// So an adapter built on this module owns one further check on both paths: reject or refuse a
/// frame above the value negotiated for *this* connection, using the limits `CapabilitiesPage`
/// reports and, for BLE streams, the established SDU.
public enum WireLimits {
    public static let magic: [UInt8] = Array("OBCP".utf8)
    public static let major: UInt8 = 3
    public static let minor: UInt8 = 0

    public static let controlHeaderBytes = 16
    public static let streamHeaderBytes = 16

    public static let minimumControlFrame = 192
    public static let maximumControlFrame = 512
    public static let minimumStreamFrame = 64
    public static let maximumStreamFrame = 4096

    /// §2: "exact bytes after this header, at most 496".
    public static let maximumControlPayload = maximumControlFrame - controlHeaderBytes

    public static let metadataEnvelopeCeiling = 128
    public static let catalogMetadataCeiling = 96
    public static let errorTextCeiling = 64
    public static let subjectCeiling = 16
    public static let retainedResults = 64
    public static let defaultCheckpointGranule: UInt32 = 262_144

    public static let catalogPagePrefixBytes = 44
    public static let catalogEntryPrefixBytes = 36
    public static let startUploadPrefixBytes = 48

    /// §2.2: codec tests assert these arithmetically rather than as byte vectors, because no legal
    /// envelope reaches either ceiling.
    public static var catalogEntryCeilingPayload: Int {
        catalogPagePrefixBytes + catalogEntryPrefixBytes + catalogMetadataCeiling
    }
    public static var startUploadCeilingPayload: Int {
        startUploadPrefixBytes + metadataEnvelopeCeiling
    }
}

/// §5's link-kind byte. A separate namespace from `ErrorOwner` even where the numbers agree.
public enum LinkKind: UInt8, Sendable, CaseIterable {
    case ble = 1
    case usb = 2
    case test = 3
}

/// §5 subject-entry namespaces.
public enum SubjectNamespace: UInt8, Sendable, CaseIterable {
    case logicalObjectKind = 1
    case draftPartKind = 2
}

/// §5 subject operation flags.
public struct SubjectOperationFlags: OptionSet, Sendable, Hashable {
    public let rawValue: UInt16
    public init(rawValue: UInt16) { self.rawValue = rawValue }
    public static let put = SubjectOperationFlags(rawValue: 1 << 0)
    public static let get = SubjectOperationFlags(rawValue: 1 << 1)
    public static let delete = SubjectOperationFlags(rawValue: 1 << 2)
    public static let setMetadata = SubjectOperationFlags(rawValue: 1 << 3)
    public static let resumableUpload = SubjectOperationFlags(rawValue: 1 << 4)
    public static let resumableDownload = SubjectOperationFlags(rawValue: 1 << 5)
    public static let draftFinalize = SubjectOperationFlags(rawValue: 1 << 6)
    static let defined: UInt16 = 0x7F
}

/// §5 subject policy flags.
public struct SubjectPolicyFlags: OptionSet, Sendable, Hashable {
    public let rawValue: UInt16
    public init(rawValue: UInt16) { self.rawValue = rawValue }
    public static let usbRecommended = SubjectPolicyFlags(rawValue: 1 << 0)
    public static let externalPowerRequired = SubjectPolicyFlags(rawValue: 1 << 1)
    public static let authenticatedPrincipalRequired = SubjectPolicyFlags(rawValue: 1 << 2)
    public static let fixedSingleton = SubjectPolicyFlags(rawValue: 1 << 3)
    static let defined: UInt16 = 0x0F
}

extension ObjectKind {
    /// Registries §1: the lifecycle table fixes which operation flags a kind may advertise. "A `no`
    /// is normative: a device that advertises it is nonconforming." The two resumable bits are
    /// device policy, so they are permitted for every kind.
    var permittedOperationFlags: SubjectOperationFlags {
        let resumable: SubjectOperationFlags = [.resumableUpload, .resumableDownload]
        switch self {
        case .route: return [.put, .get, .delete, .setMetadata, resumable]
        case .trip: return [.put, .get, .delete, resumable]
        case .ride: return [.get, .delete, resumable]
        case .weather: return [.put, .get, .delete, resumable]
        case .volumeManifest: return [.get, .delete, .setMetadata, .draftFinalize, resumable]
        case .updatePackage: return [.put, .get, .delete, resumable]
        }
    }
}

/// Registries §4: the three schema-version constants. Not a negotiation.
public enum SchemaClass: Sendable, Hashable {
    case put
    case patch
    case catalogProjection

    public var version: UInt8 {
        switch self {
        case .put: return 1
        case .patch: return 128
        case .catalogProjection: return 64
        }
    }

    /// §2.2: Put and patch envelopes are at most 128 bytes, catalog envelopes at most 96.
    public var envelopeCeiling: Int {
        self == .catalogProjection
            ? WireLimits.catalogMetadataCeiling : WireLimits.metadataEnvelopeCeiling
    }

    public init?(version: UInt8) {
        switch version {
        case 1: self = .put
        case 128: self = .patch
        case 64: self = .catalogProjection
        default: return nil
        }
    }
}
