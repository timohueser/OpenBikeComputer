/**
 * Canonical intent, Device_Object_Protocol_v3.md §11.
 *
 * The digest is what makes an OperationId replay safe: the same principal reissuing the same ID
 * with the *same* digest resumes or replays, and with a different digest gets `operationIdConflict`
 * without mutation. So the encoding has to be canonical in the strong sense — "inactive target
 * fields are included as their required zero bytes, so there is one encoding per intent", and
 * resume policy, RequestId, SessionId, connection, transport, chunking and human text are all
 * excluded because none of them changes what the operation means.
 *
 * Full SHA-256 is the equality authority; §11 forbids CRC or a truncated digest.
 */

import { Writer } from "./bytes";
import { OPCODE, type Opcode, type OpcodeName } from "./frame";
import type { StoreId } from "./ids";
import { encodeMetadataEnvelope } from "./metadata";
import type {
    AbortOperationRequest,
    BeginDraftRequest,
    MutationTarget,
    OperationOnObject,
    SetMetadataRequest,
    StartDraftPartRequest,
    StartUploadRequest,
} from "./messages";

/** `OBC-DOS3-INTENT` plus one `00` byte. DOS = Device Object System; `3` = wire major 3. */
export const INTENT_TAG = Uint8Array.of(
    0x4f, 0x42, 0x43, 0x2d, 0x44, 0x4f, 0x53, 0x33, 0x2d, 0x49, 0x4e, 0x54, 0x45, 0x4e, 0x54, 0x00,
);
export const INTENT_PREFIX_BYTES = 36;
export const INTENT_CODEC_VERSION = 1;

const OBJECT_KIND_CODE: Readonly<Record<string, number>> = {
    route: 1,
    trip: 2,
    ride: 3,
    weather: 4,
    volumeManifest: 6,
    updatePackage: 7,
};

const DRAFT_PART_KIND_CODE: Readonly<Record<string, number>> = {
    standaloneMapBlob: 1,
    mapShard: 2,
    terrainBlob: 3,
    volumeIndex: 4,
};

/** The update-package and ride ObjectKind values §11 pins literally in two of the suffixes. */
export const UPDATE_PACKAGE_KIND = 7;
export const RIDE_KIND = 3;

/** The one 36-byte prefix every wire-initiated intent begins with. */
export function intentPrefix(store: StoreId, opcode: Opcode): Uint8Array {
    return new Writer(INTENT_PREFIX_BYTES)
        .raw(INTENT_TAG)
        .raw(store)
        .u16(opcode)
        .u8(INTENT_CODEC_VERSION)
        .u8(0)
        .finish();
}

/**
 * The mutating requests §11's suffix table covers. FinalizeDraft is deliberately absent: it makes
 * no second claim, computes no canonical intent, and addresses the BeginDraft parent by OperationId
 * alone.
 */
export type IntentSource =
    | { readonly opcode: "StartUpload"; readonly request: StartUploadRequest }
    | { readonly opcode: "BeginDraft"; readonly request: BeginDraftRequest }
    | { readonly opcode: "StartDraftPart"; readonly request: StartDraftPartRequest }
    | { readonly opcode: "DeleteObject"; readonly request: MutationTarget }
    | { readonly opcode: "SetMetadata"; readonly request: SetMetadataRequest }
    | { readonly opcode: "AbortOperation"; readonly request: AbortOperationRequest }
    | { readonly opcode: "InstallUpdate"; readonly request: OperationOnObject }
    | { readonly opcode: "AcknowledgeRideImported"; readonly request: OperationOnObject };

export const INTENT_OPCODES: readonly OpcodeName[] = [
    "StartUpload",
    "BeginDraft",
    "StartDraftPart",
    "DeleteObject",
    "SetMetadata",
    "AbortOperation",
    "InstallUpdate",
    "AcknowledgeRideImported",
];

/** Builds the complete canonical intent: the 36-byte prefix followed by this opcode's suffix. */
export function canonicalIntent(store: StoreId, source: IntentSource): Uint8Array {
    const writer = new Writer(INTENT_PREFIX_BYTES + 160);
    writer.raw(intentPrefix(store, OPCODE[source.opcode]));
    switch (source.opcode) {
        case "StartUpload": {
            const request = source.request;
            const envelope = encodeMetadataEnvelope(request.metadata);
            writer
                .u16(OBJECT_KIND_CODE[request.objectKind])
                .u8(request.targetMode)
                .u8(0)
                .u64(request.logicalObjectId)
                .u64(request.expectedRevision)
                .u64(request.declaredLength)
                .u32(request.expectedCrc)
                .u16(envelope.length)
                .raw(envelope);
            break;
        }
        case "BeginDraft": {
            const request = source.request;
            writer
                .u16(OBJECT_KIND_CODE[request.objectKind])
                .u8(request.targetMode)
                .u8(0)
                .u64(request.logicalObjectId)
                .u64(request.expectedRevision)
                .u64(request.manifestLength)
                .u32(request.manifestCrc)
                .u16(request.expectedPartCount)
                .u16(0);
            break;
        }
        case "StartDraftPart": {
            const request = source.request;
            writer
                .raw(request.parentOperationId)
                .u16(DRAFT_PART_KIND_CODE[request.draftPartKind])
                .u16(0)
                .u64(request.partKey)
                .u64(request.declaredLength)
                .u32(request.expectedCrc);
            break;
        }
        case "DeleteObject": {
            const request = source.request;
            writer
                .u16(OBJECT_KIND_CODE[request.objectKind])
                .u64(request.logicalObjectId)
                .u64(request.expectedRevision);
            break;
        }
        case "SetMetadata": {
            const request = source.request;
            const envelope = encodeMetadataEnvelope(request.metadata);
            writer
                .u16(OBJECT_KIND_CODE[request.objectKind])
                .u64(request.logicalObjectId)
                .u64(request.expectedRevision)
                .u16(envelope.length)
                .raw(envelope);
            break;
        }
        case "AbortOperation": {
            const request = source.request;
            writer.raw(request.targetOperationId).u8(request.reason).zeros(7);
            break;
        }
        case "InstallUpdate": {
            const request = source.request;
            writer.u16(UPDATE_PACKAGE_KIND).u64(request.logicalObjectId).u64(request.expectedRevision);
            break;
        }
        case "AcknowledgeRideImported": {
            const request = source.request;
            writer.u16(RIDE_KIND).u64(request.logicalObjectId).u64(request.expectedRevision);
            break;
        }
    }
    return writer.finish();
}

/**
 * The three device-local schemes the storage contract freezes. They land in the same 32-byte digest
 * field as a wire intent, and the families cannot collide by construction: the wire prefix begins
 * `OBC-DOS3-INTENT\0` and every local tag begins `O2-`.
 */
export const LOCAL_INTENT_TAGS = {
    weather: "O2-LOCAL-WX-INTENT\0",
    update: "O2-LOCAL-UPD-INTENT\0",
    import: "O2-LOCAL-IMP-INTENT\0",
} as const;

/** SHA-256 over a complete canonical intent. WebCrypto in the browser, `globalThis.crypto` in Node. */
export async function intentDigest(intent: Uint8Array): Promise<Uint8Array> {
    const subtle = globalThis.crypto?.subtle;
    if (subtle === undefined) throw new Error("SHA-256 needs WebCrypto (globalThis.crypto.subtle)");
    // Copied into a buffer of its own: `digest` wants a BufferSource backed by a plain ArrayBuffer,
    // and the caller's view may sit inside a larger (or shared) one.
    const owned = new Uint8Array(intent.length);
    owned.set(intent);
    const digest = await subtle.digest("SHA-256", owned);
    return new Uint8Array(digest);
}
