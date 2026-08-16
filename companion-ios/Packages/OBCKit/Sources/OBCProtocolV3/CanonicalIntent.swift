import CryptoKit
import Foundation

/// §11's canonical intent: the exact bytes whose SHA-256 an OperationId claim stores, and the only
/// thing a same-intent replay and an `operationIdConflict` are judged against.
///
/// Resume policy, RequestId, SessionId, connection, transport, chunks, and human text are excluded.
/// Inactive target fields are included as their required zero bytes, so there is one encoding per
/// intent. Full SHA-256 is the equality authority; CRC or a truncated digest is forbidden.
public enum CanonicalIntent {
    /// `OBC-DOS3-INTENT` plus one `00` byte. DOS = Device Object System; `3` = wire major 3.
    public static let tag: [UInt8] = Array("OBC-DOS3-INTENT".utf8) + [0]
    public static let prefixBytes = 36
    public static let codecVersion: UInt8 = 1

    /// The three device-local schemes of §11. They occupy the same 32-byte digest field, and the
    /// families cannot collide because every wire intent begins `OBC-DOS3-INTENT\0` and every local
    /// tag begins `O2-`.
    public enum LocalTag: String, Sendable, CaseIterable {
        case weather = "O2-LOCAL-WX-INTENT"
        case update = "O2-LOCAL-UPD-INTENT"
        case importStaged = "O2-LOCAL-IMP-INTENT"

        public var bytes: [UInt8] { Array(rawValue.utf8) + [0] }
    }

    /// The operations §11's canonical-suffix table covers. FinalizeDraft is deliberately absent: it
    /// makes no second claim, computes no intent, and its only request field is the parent lookup
    /// key.
    public enum Intent: Hashable, Sendable {
        case startUpload(StartUploadRequest)
        case beginDraft(BeginDraftRequest)
        case startDraftPart(StartDraftPartRequest)
        case deleteObject(DeleteObjectRequest)
        case setMetadata(SetMetadataRequest)
        case abortOperation(AbortOperationRequest)
        case installUpdate(KindedCommandRequest)
        case acknowledgeRideImported(KindedCommandRequest)

        public var opcode: Opcode {
            switch self {
            case .startUpload: return .startUpload
            case .beginDraft: return .beginDraft
            case .startDraftPart: return .startDraftPart
            case .deleteObject: return .deleteObject
            case .setMetadata: return .setMetadata
            case .abortOperation: return .abortOperation
            case .installUpdate: return .installUpdate
            case .acknowledgeRideImported: return .acknowledgeRideImported
            }
        }
    }

    /// The 36-byte prefix every wire-initiated intent begins with.
    public static func prefix(storeId: StoreId, opcode: Opcode) -> [UInt8] {
        var writer = ByteWriter()
        writer.raw(tag)
        writer.raw(storeId.bytes)
        writer.u16(opcode.rawValue)
        writer.u8(codecVersion)
        writer.u8(0)
        return writer.bytes
    }

    /// §11's per-opcode suffix, in order, with no struct padding.
    public static func suffix(_ intent: Intent) throws -> [UInt8] {
        var writer = ByteWriter()
        switch intent {
        case .startUpload(let request):
            writer.u16(request.objectKind.rawValue)
            writer.u8(request.targetMode.rawValue)
            writer.u8(0)
            writer.u64(request.logicalObjectId.rawValue)
            writer.u64(request.expectedRevision.rawValue)
            writer.u64(request.declaredLength)
            writer.u32(request.expectedCRC32)
            let envelope = try request.metadata.encoded()
            writer.u16(try narrowU16(envelope.count, "canonical intent: envelope length"))
            writer.raw(envelope)
        case .beginDraft(let request):
            writer.u16(request.finalObjectKind.rawValue)
            writer.u8(request.targetMode.rawValue)
            writer.u8(0)
            writer.u64(request.logicalObjectId.rawValue)
            writer.u64(request.expectedRevision.rawValue)
            writer.u64(request.declaredManifestLength)
            writer.u32(request.declaredManifestCRC32)
            writer.u16(request.expectedPartCount)
            writer.u16(0)
        case .startDraftPart(let request):
            writer.raw(request.parentOperationId.bytes)
            writer.u16(request.draftPartKind.rawValue)
            writer.u16(0)
            writer.u64(request.partKey.rawValue)
            writer.u64(request.declaredLength)
            writer.u32(request.expectedCRC32)
        case .deleteObject(let request):
            writer.u16(request.target.objectKind.rawValue)
            writer.u64(request.target.logicalObjectId.rawValue)
            writer.u64(request.target.expectedRevision.rawValue)
        case .setMetadata(let request):
            writer.u16(request.target.objectKind.rawValue)
            writer.u64(request.target.logicalObjectId.rawValue)
            writer.u64(request.target.expectedRevision.rawValue)
            let envelope = try request.patch.encoded()
            writer.u16(try narrowU16(envelope.count, "canonical intent: envelope length"))
            writer.raw(envelope)
        case .abortOperation(let request):
            writer.raw(request.targetOperationId.bytes)
            writer.u8(request.reason.rawValue)
            writer.zeros(7)
        case .installUpdate(let request), .acknowledgeRideImported(let request):
            // §11 spells the kind out — update `7`, ride `3` — even though the request does not
            // carry it, so the two intents cannot alias.
            writer.u16(request.impliedKind.rawValue)
            writer.u64(request.logicalObjectId.rawValue)
            writer.u64(request.expectedRevision.rawValue)
        }
        return writer.bytes
    }

    public static func bytes(storeId: StoreId, intent: Intent) throws -> [UInt8] {
        prefix(storeId: storeId, opcode: intent.opcode) + (try suffix(intent))
    }

    /// Full SHA-256 over the canonical bytes. A truncated digest is forbidden.
    public static func digest(storeId: StoreId, intent: Intent) throws -> [UInt8] {
        digest(of: try bytes(storeId: storeId, intent: intent))
    }

    public static func digest(of bytes: [UInt8]) -> [UInt8] {
        Array(SHA256.hash(data: Data(bytes)))
    }

    /// Extracts the intent from a decoded control frame, or nil when the opcode makes no claim.
    public static func intent(of frame: ControlFrame) -> Intent? {
        switch frame.body {
        case .startUpload(let request): return .startUpload(request)
        case .beginDraft(let request): return .beginDraft(request)
        case .startDraftPart(let request): return .startDraftPart(request)
        case .deleteObject(let request): return .deleteObject(request)
        case .setMetadata(let request): return .setMetadata(request)
        case .abortOperation(let request): return .abortOperation(request)
        case .installUpdate(let request): return .installUpdate(request)
        case .acknowledgeRideImported(let request): return .acknowledgeRideImported(request)
        default: return nil
        }
    }
}
