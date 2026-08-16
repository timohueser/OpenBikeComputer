/**
 * The opcode-specific payload bodies of Device_Object_Protocol_v3.md §6 through §10 and §16.
 *
 * Every layout here is fixed: §2.1 says there is no extension block and no in-band mechanism for a
 * peer to attach a field this document does not define, so "a frame that carries a byte past the
 * end of its stated layout is `invalidFrame` for the trailing bytes it contains." That is why each
 * decoder ends with an explicit end-of-body check rather than tolerating a longer body, and why the
 * three resumable acceptances reject their own pre-freeze sizes instead of decoding short.
 */

import { Cursor, Writer } from "./bytes";
import { decodeErrorBody, encodeErrorBody, type ErrorBody } from "./errorBody";
import {
    draftPartRef,
    identityEquals,
    identityIsZero,
    logicalObjectId,
    operationId,
    pageCursor,
    partKey,
    revision,
    draftRevision as makeDraftRevision,
    storeId,
    deviceSerial,
    weatherRequestId,
    type DeviceSerial,
    type DraftPartRef,
    type DraftRevision,
    type LogicalObjectId,
    type OperationId,
    type PageCursor,
    type PartKey,
    type Revision,
    type StoreId,
    type WeatherRequestId,
} from "./ids";
import { decodeMetadataEnvelope, decodeWireText, encodeMetadataEnvelope, type MetadataEnvelope } from "./metadata";
import {
    DRAFT_PART_KIND_NAME,
    OBJECT_KIND_NAME,
    type DraftPartKindName,
    type ObjectKindName,
} from "./registry";
import { reject } from "./result";

/** §6.1 target mode. Zero is not a sentinel in either identity field; the mode alone distinguishes. */
export const TARGET_MODE = { create: 0, replace: 1 } as const;
/** §6.1 resume preference. A preference, not a demand, with exactly two legal values. */
export const RESUME = { restartAtZero: 0, permitted: 1 } as const;
/** §6.1 acceptance flags, carried identically by all three resumable acceptances. */
export const ACCEPT_FLAG = { resumedWork: 1 << 0, restartAtZero: 1 << 1 } as const;
const ACCEPT_FLAG_MASK = 0x03;

/** §6.4 abort reasons, shared by AbortSession and AbortOperation. */
export const ABORT_REASON = { clientCancelled: 1, superseded: 2, userRequested: 3 } as const;

/** §10 typed result bodies. */
export const RESULT_TYPE = { objectResult: 1, draftPartResult: 2, abortResult: 3 } as const;

/** §10 ObjectResult outcomes. `1` is registered, reserved, and never emitted in v3.0. */
export const OUTCOME = {
    committed: 0,
    committedSupersededWeather: 1,
    deleted: 2,
    metadataChanged: 3,
    updateInstallRequested: 4,
    rideImported: 5,
} as const;
const MAX_OUTCOME = 5;

/** §8.1 operation phases, projected from the storage contract's phase names. */
export const PHASE = {
    prepared: 0,
    streaming: 1,
    sealed: 2,
    validating: 3,
    publishing: 4,
    externalHandoff: 5,
    draftOpen: 6,
    aborting: 7,
} as const;
const MAX_PHASE = 7;

/** §8.1 progress flags. */
export const PROGRESS_FLAG = { resumable: 1 << 0, sessionAttached: 1 << 1, logicalIdPresent: 1 << 2 } as const;
const PROGRESS_FLAG_MASK = 0x07;

/** §8.1 QueryOperation states. */
export const OPERATION_STATE = { unknown: 0, inProgress: 1, committed: 2, aborted: 3 } as const;

/** §8.3 draft part states. */
export const DRAFT_PART_STATE = { prepared: 0, streaming: 1, sealed: 2, aborted: 3 } as const;

/** §16 mount classes, reproducing the storage contract's classification. */
export const MOUNT_CLASS = {
    noCard: 0,
    unsupportedFilesystem: 1,
    initializing: 2,
    mounted: 3,
    mountedDegradedEntry: 4,
    recoveryFailedReadOnly: 5,
    mountedStoreWideDegraded: 6,
} as const;
const MAX_MOUNT_CLASS = 6;
const CLASSES_REPORTING_A_STORE: readonly number[] = [
    MOUNT_CLASS.mounted,
    MOUNT_CLASS.mountedDegradedEntry,
    MOUNT_CLASS.mountedStoreWideDegraded,
];

export const CONFIG_BLOCK_BYTES = 56;
export const CONFIG_CODEC_VERSION = 1;
export const MAX_DEVICE_NAME_BYTES = 32;
const UNIT_FLAG_MASK = 0x07;
const MAX_WEATHER_REFRESH = 4;

export const CLOCK_SOURCE = { companion: 1, gps: 2 } as const;
export const CLOCK_STATE = { untrusted: 0, trusted: 1 } as const;
export const FORGET_BOND_SCOPE = { thisBond: 1, everyBond: 2 } as const;

const objectKindOf = (code: number): ObjectKindName => {
    const kind = OBJECT_KIND_NAME.get(code);
    if (kind === undefined) reject("invalidDescriptor", "unknownEnum", `ObjectKind ${code} is never encoded`);
    return kind;
};

const draftPartKindOf = (code: number): DraftPartKindName => {
    const kind = DRAFT_PART_KIND_NAME.get(code);
    if (kind === undefined) reject("invalidDescriptor", "unknownEnum", `DraftPartKind ${code} is not registered`);
    return kind;
};

const nonzeroSession = (value: number): number => {
    if (value === 0) reject("invalidDescriptor", "unknownEnum", "a SessionId is nonzero");
    return value;
};

function enumIn(value: number, allowed: readonly number[], what: string): number {
    if (!allowed.includes(value)) reject("invalidDescriptor", "unknownEnum", `${what} value ${value} is not registered`);
    return value;
}

// ---------------------------------------------------------------------------- typed results (§10)

export interface ObjectResult {
    readonly operationId: OperationId;
    readonly storeId: StoreId;
    readonly objectKind: ObjectKindName;
    readonly outcome: number;
    readonly logicalObjectId: LogicalObjectId;
    readonly newRevision: Revision;
    readonly length: bigint;
    readonly crc: number;
}

export interface DraftPartResult {
    readonly childOperationId: OperationId;
    readonly storeId: StoreId;
    readonly parentOperationId: OperationId;
    readonly draftPartRef: DraftPartRef;
    readonly draftPartKind: DraftPartKindName;
    readonly partKey: PartKey;
    readonly length: bigint;
    readonly crc: number;
}

export interface AbortResult {
    readonly abortOperationId: OperationId;
    readonly storeId: StoreId;
    readonly targetOperationId: OperationId;
    readonly disposition: number;
}

export type ResultEnvelope =
    | { readonly type: "objectResult"; readonly result: ObjectResult }
    | { readonly type: "draftPartResult"; readonly result: DraftPartResult }
    | { readonly type: "abortResult"; readonly result: AbortResult };

/**
 * §10: the envelope carries no body length because it is always the final element of the payload,
 * so a decoder takes the remainder of the frame and rejects any trailing byte beyond the fixed size.
 */
export function decodeResultEnvelope(bytes: Uint8Array): ResultEnvelope {
    const cursor = new Cursor(bytes);
    const type = cursor.u8();
    cursor.zeros(3, "ResultEnvelope reserved bytes");
    const body = cursor.take(cursor.remaining);
    switch (type) {
        case RESULT_TYPE.objectResult:
            return { type: "objectResult", result: decodeObjectResult(body) };
        case RESULT_TYPE.draftPartResult:
            return { type: "draftPartResult", result: decodeDraftPartResult(body) };
        case RESULT_TYPE.abortResult:
            return { type: "abortResult", result: decodeAbortResult(body) };
        default:
            return reject("invalidDescriptor", "unknownEnum", `result type ${type} is not registered`);
    }
}

export function encodeResultEnvelope(envelope: ResultEnvelope): Uint8Array {
    const writer = new Writer(96);
    switch (envelope.type) {
        case "objectResult":
            writer.u8(RESULT_TYPE.objectResult).zeros(3).raw(encodeObjectResult(envelope.result));
            break;
        case "draftPartResult":
            writer.u8(RESULT_TYPE.draftPartResult).zeros(3).raw(encodeDraftPartResult(envelope.result));
            break;
        case "abortResult":
            writer.u8(RESULT_TYPE.abortResult).zeros(3).raw(encodeAbortResult(envelope.result));
            break;
    }
    return writer.finish();
}

function decodeObjectResult(bytes: Uint8Array): ObjectResult {
    const cursor = new Cursor(bytes);
    const result: ObjectResult = {
        operationId: operationId(cursor.take(16)),
        storeId: storeId(cursor.take(16)),
        objectKind: objectKindOf(cursor.u16()),
        outcome: cursor.u16(),
        logicalObjectId: logicalObjectId(cursor.u64()),
        newRevision: revision(cursor.u64()),
        length: cursor.u64(),
        crc: cursor.u32(),
    };
    cursor.end("ObjectResult");
    if (result.outcome > MAX_OUTCOME) {
        reject("invalidDescriptor", "unknownEnum", `ObjectResult outcome ${result.outcome} is not registered`);
    }
    return result;
}

function encodeObjectResult(result: ObjectResult): Uint8Array {
    return new Writer(64)
        .raw(result.operationId)
        .raw(result.storeId)
        .u16(OBJECT_KIND_CODE[result.objectKind])
        .u16(result.outcome)
        .u64(result.logicalObjectId)
        .u64(result.newRevision)
        .u64(result.length)
        .u32(result.crc)
        .finish();
}

function decodeDraftPartResult(bytes: Uint8Array): DraftPartResult {
    const cursor = new Cursor(bytes);
    const childOperationId = operationId(cursor.take(16));
    const store = storeId(cursor.take(16));
    const parentOperationId = operationId(cursor.take(16));
    const ref = draftPartRef(cursor.take(16));
    const kind = draftPartKindOf(cursor.u16());
    cursor.zeros(2, "DraftPartResult byte 66");
    const key = partKey(cursor.u64());
    const length = cursor.u64();
    const crc = cursor.u32();
    cursor.end("DraftPartResult");
    return {
        childOperationId,
        storeId: store,
        parentOperationId,
        draftPartRef: ref,
        draftPartKind: kind,
        partKey: key,
        length,
        crc,
    };
}

function encodeDraftPartResult(result: DraftPartResult): Uint8Array {
    return new Writer(88)
        .raw(result.childOperationId)
        .raw(result.storeId)
        .raw(result.parentOperationId)
        .raw(result.draftPartRef)
        .u16(DRAFT_PART_KIND_CODE[result.draftPartKind])
        .zeros(2)
        .u64(result.partKey)
        .u64(result.length)
        .u32(result.crc)
        .finish();
}

function decodeAbortResult(bytes: Uint8Array): AbortResult {
    const cursor = new Cursor(bytes);
    const abortOperationId = operationId(cursor.take(16));
    const store = storeId(cursor.take(16));
    const targetOperationId = operationId(cursor.take(16));
    const disposition = cursor.u8();
    cursor.zeros(7, "AbortResult reserved bytes");
    cursor.end("AbortResult");
    if (disposition > 2) {
        reject("invalidDescriptor", "unknownEnum", `AbortResult disposition ${disposition} is not registered`);
    }
    return { abortOperationId, storeId: store, targetOperationId, disposition };
}

function encodeAbortResult(result: AbortResult): Uint8Array {
    return new Writer(56)
        .raw(result.abortOperationId)
        .raw(result.storeId)
        .raw(result.targetOperationId)
        .u8(result.disposition)
        .zeros(7)
        .finish();
}

const OBJECT_KIND_CODE: Readonly<Record<ObjectKindName, number>> = {
    route: 1,
    trip: 2,
    ride: 3,
    weather: 4,
    volumeManifest: 6,
    updatePackage: 7,
};

const DRAFT_PART_KIND_CODE: Readonly<Record<DraftPartKindName, number>> = {
    standaloneMapBlob: 1,
    mapShard: 2,
    terrainBlob: 3,
    volumeIndex: 4,
};

// ------------------------------------------------------------------------------- uploads (§6.1–3)

export interface StartUploadRequest {
    readonly operationId: OperationId;
    readonly objectKind: ObjectKindName;
    readonly targetMode: number;
    readonly resume: number;
    readonly logicalObjectId: LogicalObjectId;
    readonly expectedRevision: Revision;
    readonly declaredLength: bigint;
    readonly expectedCrc: number;
    readonly metadata: MetadataEnvelope;
}

export function decodeStartUpload(bytes: Uint8Array): StartUploadRequest {
    const cursor = new Cursor(bytes);
    const id = operationId(cursor.take(16));
    const objectKind = objectKindOf(cursor.u16());
    const targetMode = enumIn(cursor.u8(), [TARGET_MODE.create, TARGET_MODE.replace], "target mode");
    const resume = enumIn(cursor.u8(), [RESUME.restartAtZero, RESUME.permitted], "resume");
    const logical = logicalObjectId(cursor.u64());
    const expected = revision(cursor.u64());
    const declaredLength = cursor.u64();
    const expectedCrc = cursor.u32();
    if (targetMode === TARGET_MODE.create && (logical !== 0n || expected !== 0n)) {
        reject(
            "invalidDescriptor",
            "invalidCombination",
            "create encodes logical ID and expected revision as zero",
        );
    }
    const metadata = decodeMetadataEnvelope(bytes.subarray(cursor.position), {
        kind: objectKind,
        role: "put",
        mutating: true,
    });
    if (bytes.length !== cursor.position + metadata.byteLength) {
        reject("invalidFrame", "trailingBytes", "StartUpload ends with exactly one metadata envelope");
    }
    return {
        operationId: id,
        objectKind,
        targetMode,
        resume,
        logicalObjectId: logical,
        expectedRevision: expected,
        declaredLength,
        expectedCrc,
        metadata,
    };
}

export function encodeStartUpload(request: StartUploadRequest): Uint8Array {
    return new Writer(176)
        .raw(request.operationId)
        .u16(OBJECT_KIND_CODE[request.objectKind])
        .u8(request.targetMode)
        .u8(request.resume)
        .u64(request.logicalObjectId)
        .u64(request.expectedRevision)
        .u64(request.declaredLength)
        .u32(request.expectedCrc)
        .raw(encodeMetadataEnvelope(request.metadata))
        .finish();
}

export interface UploadAcceptance {
    readonly disposition: 0;
    readonly targetMode: number;
    readonly flags: number;
    readonly operationId: OperationId;
    readonly sessionId: number;
    readonly logicalObjectId: LogicalObjectId;
    readonly repositoryRevision: Revision;
    readonly durableNextOffset: bigint;
    readonly checkpointGranule: number;
    readonly maximumStreamPayload: number;
    readonly finalizedPrefixCrc: number;
}

export type DispositionResponse<T> =
    | { readonly disposition: "accepted"; readonly accepted: T }
    | { readonly disposition: "alreadyTerminal"; readonly result: ResultEnvelope };

/**
 * §6.1: acceptance flags are a `u16` at offset 2 in all three resumable acceptances, and the
 * restart-at-zero and resumed-work bits are never both set. Restart-at-zero forces the reported
 * durable next offset and the finalized prefix CRC to zero, and the CRC is zero at offset zero —
 * "the only case in which zero is not a computed CRC".
 */
function checkAcceptanceFlags(flags: number, durableOffset: bigint, prefixCrc: number): void {
    if ((flags & ~ACCEPT_FLAG_MASK) !== 0) {
        reject("invalidDescriptor", "reservedBits", "acceptance flags above bit 1 are zero");
    }
    if ((flags & ACCEPT_FLAG.resumedWork) !== 0 && (flags & ACCEPT_FLAG.restartAtZero) !== 0) {
        reject("invalidDescriptor", "invalidCombination", "restart-at-zero and resumed-work are never both set");
    }
    if ((flags & ACCEPT_FLAG.restartAtZero) !== 0 && durableOffset !== 0n) {
        reject("invalidDescriptor", "invalidCombination", "restart-at-zero forces the durable next offset to zero");
    }
    if (durableOffset === 0n && prefixCrc !== 0) {
        reject("invalidDescriptor", "invalidCombination", "the finalized prefix CRC is zero over an empty prefix");
    }
}

export function decodeUploadAccepted(bytes: Uint8Array): DispositionResponse<UploadAcceptance> {
    const cursor = new Cursor(bytes);
    const disposition = cursor.u8();
    if (disposition === 1) return { disposition: "alreadyTerminal", result: decodeTerminalDisposition(cursor, bytes) };
    if (disposition !== 0) {
        reject("invalidDescriptor", "unknownEnum", `disposition ${disposition} is not registered`);
    }
    const targetMode = enumIn(cursor.u8(), [TARGET_MODE.create, TARGET_MODE.replace], "target mode");
    const flags = cursor.u16();
    const id = operationId(cursor.take(16));
    const session = nonzeroSession(cursor.u32());
    const logical = logicalObjectId(cursor.u64());
    const repositoryRevision = revision(cursor.u64());
    const durableNextOffset = cursor.u64();
    const checkpointGranule = cursor.u32();
    const maximumStreamPayload = cursor.u16();
    cursor.zeros(2, "UploadAccepted byte 54");
    const finalizedPrefixCrc = cursor.u32();
    cursor.zeros(4, "UploadAccepted byte 60");
    cursor.end("UploadAccepted");
    checkAcceptanceFlags(flags, durableNextOffset, finalizedPrefixCrc);
    return {
        disposition: "accepted",
        accepted: {
            disposition: 0,
            targetMode,
            flags,
            operationId: id,
            sessionId: session,
            logicalObjectId: logical,
            repositoryRevision,
            durableNextOffset,
            checkpointGranule,
            maximumStreamPayload,
            finalizedPrefixCrc,
        },
    };
}

export function encodeUploadAccepted(response: DispositionResponse<UploadAcceptance>): Uint8Array {
    if (response.disposition === "alreadyTerminal") return encodeTerminalDisposition(response.result);
    const accepted = response.accepted;
    return new Writer(64)
        .u8(0)
        .u8(accepted.targetMode)
        .u16(accepted.flags)
        .raw(accepted.operationId)
        .u32(accepted.sessionId)
        .u64(accepted.logicalObjectId)
        .u64(accepted.repositoryRevision)
        .u64(accepted.durableNextOffset)
        .u32(accepted.checkpointGranule)
        .u16(accepted.maximumStreamPayload)
        .zeros(2)
        .u32(accepted.finalizedPrefixCrc)
        .zeros(4)
        .finish();
}

function decodeTerminalDisposition(cursor: Cursor, bytes: Uint8Array): ResultEnvelope {
    cursor.zeros(3, "disposition reserved bytes");
    return decodeResultEnvelope(bytes.subarray(cursor.position));
}

function encodeTerminalDisposition(result: ResultEnvelope): Uint8Array {
    return new Writer(96).u8(1).zeros(3).raw(encodeResultEnvelope(result)).finish();
}

export interface CheckpointUploadRequest {
    readonly sessionId: number;
    readonly receivedNextOffset: bigint;
}

export function decodeCheckpointUpload(bytes: Uint8Array): CheckpointUploadRequest {
    const cursor = new Cursor(bytes);
    const sessionId = nonzeroSession(cursor.u32());
    const receivedNextOffset = cursor.u64();
    cursor.end("CheckpointUpload");
    return { sessionId, receivedNextOffset };
}

export const encodeCheckpointUpload = (request: CheckpointUploadRequest): Uint8Array =>
    new Writer(12).u32(request.sessionId).u64(request.receivedNextOffset).finish();

export interface CheckpointUploadResponse {
    readonly sessionId: number;
    readonly durableNextOffset: bigint;
    readonly finalizedPrefixCrc: number;
    readonly checkpointSequence: number;
}

export function decodeCheckpointResponse(bytes: Uint8Array): CheckpointUploadResponse {
    const cursor = new Cursor(bytes);
    const sessionId = nonzeroSession(cursor.u32());
    const durableNextOffset = cursor.u64();
    const finalizedPrefixCrc = cursor.u32();
    const checkpointSequence = cursor.u32();
    cursor.end("checkpoint response");
    return { sessionId, durableNextOffset, finalizedPrefixCrc, checkpointSequence };
}

export const encodeCheckpointResponse = (response: CheckpointUploadResponse): Uint8Array =>
    new Writer(20)
        .u32(response.sessionId)
        .u64(response.durableNextOffset)
        .u32(response.finalizedPrefixCrc)
        .u32(response.checkpointSequence)
        .finish();

export interface SessionOnlyRequest {
    readonly sessionId: number;
}

export function decodeSessionOnly(bytes: Uint8Array, what: string): SessionOnlyRequest {
    const cursor = new Cursor(bytes);
    const sessionId = nonzeroSession(cursor.u32());
    cursor.end(what);
    return { sessionId };
}

export const encodeSessionOnly = (request: SessionOnlyRequest): Uint8Array => new Writer(4).u32(request.sessionId).finish();

// ------------------------------------------------------------------------------- downloads (§7)

export const DOWNLOAD_FLAG = { reservedRevision: 1 << 0, startOffset: 1 << 1 } as const;
const DOWNLOAD_FLAG_MASK = 0x02;

export interface StartDownloadRequest {
    readonly objectKind: ObjectKindName;
    readonly flags: number;
    readonly logicalObjectId: LogicalObjectId;
    readonly startOffset: bigint;
}

export function decodeStartDownload(bytes: Uint8Array): StartDownloadRequest {
    const cursor = new Cursor(bytes);
    const objectKind = objectKindOf(cursor.u16());
    const flags = cursor.u16();
    const logical = logicalObjectId(cursor.u64());
    cursor.zeros(8, "StartDownload bytes 12..19");
    const startOffset = cursor.u64();
    cursor.end("StartDownload");
    if ((flags & ~DOWNLOAD_FLAG_MASK) !== 0) {
        reject("invalidDescriptor", "reservedBits", "the requested-revision flag and bits 2..15 are burned and zero");
    }
    if ((flags & DOWNLOAD_FLAG.startOffset) === 0 && startOffset !== 0n) {
        reject("invalidDescriptor", "reservedBits", "an inactive start offset is encoded zero");
    }
    return { objectKind, flags, logicalObjectId: logical, startOffset };
}

export const encodeStartDownload = (request: StartDownloadRequest): Uint8Array =>
    new Writer(28)
        .u16(OBJECT_KIND_CODE[request.objectKind])
        .u16(request.flags)
        .u64(request.logicalObjectId)
        .zeros(8)
        .u64(request.startOffset)
        .finish();

export interface DownloadAccepted {
    readonly storeId: StoreId;
    readonly sessionId: number;
    readonly logicalObjectId: LogicalObjectId;
    readonly pinnedRevision: Revision;
    readonly totalLength: bigint;
    readonly wholeSourceCrc: number;
    readonly acceptedStartOffset: bigint;
    readonly maximumStreamPayload: number;
}

export function decodeDownloadAccepted(bytes: Uint8Array): DownloadAccepted {
    const cursor = new Cursor(bytes);
    const store = storeId(cursor.take(16));
    const sessionId = nonzeroSession(cursor.u32());
    const logical = logicalObjectId(cursor.u64());
    const pinnedRevision = revision(cursor.u64());
    const totalLength = cursor.u64();
    const wholeSourceCrc = cursor.u32();
    const acceptedStartOffset = cursor.u64();
    const maximumStreamPayload = cursor.u16();
    cursor.zeros(2, "DownloadAccepted byte 58");
    cursor.end("DownloadAccepted");
    return {
        storeId: store,
        sessionId,
        logicalObjectId: logical,
        pinnedRevision,
        totalLength,
        wholeSourceCrc,
        acceptedStartOffset,
        maximumStreamPayload,
    };
}

export const encodeDownloadAccepted = (response: DownloadAccepted): Uint8Array =>
    new Writer(60)
        .raw(response.storeId)
        .u32(response.sessionId)
        .u64(response.logicalObjectId)
        .u64(response.pinnedRevision)
        .u64(response.totalLength)
        .u32(response.wholeSourceCrc)
        .u64(response.acceptedStartOffset)
        .u16(response.maximumStreamPayload)
        .zeros(2)
        .finish();

export interface FinishDownloadRequest {
    readonly sessionId: number;
    readonly receivedLength: bigint;
    readonly wholeSourceCrc: number;
}

export function decodeFinishDownload(bytes: Uint8Array): FinishDownloadRequest {
    const cursor = new Cursor(bytes);
    const sessionId = nonzeroSession(cursor.u32());
    const receivedLength = cursor.u64();
    const wholeSourceCrc = cursor.u32();
    cursor.end("FinishDownload");
    return { sessionId, receivedLength, wholeSourceCrc };
}

export const encodeFinishDownload = (request: FinishDownloadRequest): Uint8Array =>
    new Writer(16).u32(request.sessionId).u64(request.receivedLength).u32(request.wholeSourceCrc).finish();

// ------------------------------------------------------------------------------- aborts (§6.4)

export interface AbortSessionRequest {
    readonly sessionId: number;
    readonly reason: number;
}

export function decodeAbortSession(bytes: Uint8Array): AbortSessionRequest {
    const cursor = new Cursor(bytes);
    const sessionId = nonzeroSession(cursor.u32());
    const reason = enumIn(cursor.u8(), [1, 2, 3], "abort reason");
    cursor.zeros(3, "AbortSession reserved bytes");
    cursor.end("AbortSession");
    return { sessionId, reason };
}

export const encodeAbortSession = (request: AbortSessionRequest): Uint8Array =>
    new Writer(8).u32(request.sessionId).u8(request.reason).zeros(3).finish();

export interface AbortSessionResponse {
    /** `0` detached, `1` already terminal. */
    readonly outcome: number;
}

export function decodeAbortSessionResponse(bytes: Uint8Array): AbortSessionResponse {
    const cursor = new Cursor(bytes);
    const outcome = enumIn(cursor.u8(), [0, 1], "AbortSession outcome");
    cursor.end("AbortSession response");
    return { outcome };
}

export const encodeAbortSessionResponse = (response: AbortSessionResponse): Uint8Array =>
    new Writer(1).u8(response.outcome).finish();

export interface AbortOperationRequest {
    readonly operationId: OperationId;
    readonly targetOperationId: OperationId;
    readonly reason: number;
}

export function decodeAbortOperation(bytes: Uint8Array): AbortOperationRequest {
    const cursor = new Cursor(bytes);
    const id = operationId(cursor.take(16));
    const target = operationId(cursor.take(16));
    const reason = enumIn(cursor.u8(), [1, 2, 3], "abort reason");
    cursor.zeros(7, "AbortOperation reserved bytes");
    cursor.end("AbortOperation");
    return { operationId: id, targetOperationId: target, reason };
}

export const encodeAbortOperation = (request: AbortOperationRequest): Uint8Array =>
    new Writer(40).raw(request.operationId).raw(request.targetOperationId).u8(request.reason).zeros(7).finish();

// ------------------------------------------------------------------------------- drafts (§6.5)

export const MAX_DRAFT_PARTS = 32;

export interface BeginDraftRequest {
    readonly parentOperationId: OperationId;
    readonly objectKind: ObjectKindName;
    readonly targetMode: number;
    readonly logicalObjectId: LogicalObjectId;
    readonly expectedRevision: Revision;
    readonly manifestLength: bigint;
    readonly manifestCrc: number;
    readonly expectedPartCount: number;
}

export function decodeBeginDraft(bytes: Uint8Array): BeginDraftRequest {
    const cursor = new Cursor(bytes);
    const parent = operationId(cursor.take(16));
    const objectKind = objectKindOf(cursor.u16());
    const targetMode = enumIn(cursor.u8(), [TARGET_MODE.create, TARGET_MODE.replace], "target mode");
    cursor.zeros(1, "BeginDraft byte 19");
    const logical = logicalObjectId(cursor.u64());
    const expected = revision(cursor.u64());
    const manifestLength = cursor.u64();
    const manifestCrc = cursor.u32();
    const expectedPartCount = cursor.u16();
    cursor.zeros(2, "BeginDraft byte 50");
    cursor.end("BeginDraft");
    if (targetMode === TARGET_MODE.create && (logical !== 0n || expected !== 0n)) {
        reject("invalidDescriptor", "invalidCombination", "create encodes logical ID and expected revision as zero");
    }
    if (expectedPartCount === 0 || expectedPartCount > MAX_DRAFT_PARTS) {
        reject(
            "invalidDescriptor",
            "invalidCombination",
            `the exact part count is 1 through ${MAX_DRAFT_PARTS}, not ${expectedPartCount}`,
        );
    }
    return {
        parentOperationId: parent,
        objectKind,
        targetMode,
        logicalObjectId: logical,
        expectedRevision: expected,
        manifestLength,
        manifestCrc,
        expectedPartCount,
    };
}

export const encodeBeginDraft = (request: BeginDraftRequest): Uint8Array =>
    new Writer(52)
        .raw(request.parentOperationId)
        .u16(OBJECT_KIND_CODE[request.objectKind])
        .u8(request.targetMode)
        .zeros(1)
        .u64(request.logicalObjectId)
        .u64(request.expectedRevision)
        .u64(request.manifestLength)
        .u32(request.manifestCrc)
        .u16(request.expectedPartCount)
        .zeros(2)
        .finish();

export interface BeginDraftAcceptance {
    readonly parentOperationId: OperationId;
    readonly draftRevision: DraftRevision;
    readonly expectedParts: number;
    readonly state: number;
}

export function decodeBeginDraftResponse(bytes: Uint8Array): DispositionResponse<BeginDraftAcceptance> {
    const cursor = new Cursor(bytes);
    const disposition = cursor.u8();
    if (disposition === 1) return { disposition: "alreadyTerminal", result: decodeTerminalDisposition(cursor, bytes) };
    if (disposition !== 0) reject("invalidDescriptor", "unknownEnum", `disposition ${disposition} is not registered`);
    cursor.zeros(3, "BeginDraft disposition reserved bytes");
    const parent = operationId(cursor.take(16));
    const revisionValue = makeDraftRevision(cursor.u64());
    const expectedParts = cursor.u16();
    const state = enumIn(cursor.u8(), [0], "draft state");
    cursor.zeros(1, "BeginDraft response byte 31");
    cursor.end("BeginDraft response");
    return {
        disposition: "accepted",
        accepted: { parentOperationId: parent, draftRevision: revisionValue, expectedParts, state },
    };
}

export function encodeBeginDraftResponse(response: DispositionResponse<BeginDraftAcceptance>): Uint8Array {
    if (response.disposition === "alreadyTerminal") return encodeTerminalDisposition(response.result);
    const accepted = response.accepted;
    return new Writer(32)
        .u8(0)
        .zeros(3)
        .raw(accepted.parentOperationId)
        .u64(accepted.draftRevision)
        .u16(accepted.expectedParts)
        .u8(accepted.state)
        .zeros(1)
        .finish();
}

export interface StartDraftPartRequest {
    readonly childOperationId: OperationId;
    readonly parentOperationId: OperationId;
    readonly draftPartKind: DraftPartKindName;
    readonly partKey: PartKey;
    readonly declaredLength: bigint;
    readonly expectedCrc: number;
    readonly resume: number;
}

export function decodeStartDraftPart(bytes: Uint8Array): StartDraftPartRequest {
    const cursor = new Cursor(bytes);
    const child = operationId(cursor.take(16));
    const parent = operationId(cursor.take(16));
    const kind = draftPartKindOf(cursor.u16());
    cursor.zeros(2, "StartDraftPart byte 34");
    const key = partKey(cursor.u64());
    const declaredLength = cursor.u64();
    const expectedCrc = cursor.u32();
    const resume = enumIn(cursor.u8(), [RESUME.restartAtZero, RESUME.permitted], "resume");
    cursor.zeros(7, "StartDraftPart byte 57");
    cursor.end("StartDraftPart");
    if (identityEquals(child, parent)) {
        reject("invalidDescriptor", "invalidCombination", "the child OperationId must differ from the parent");
    }
    return {
        childOperationId: child,
        parentOperationId: parent,
        draftPartKind: kind,
        partKey: key,
        declaredLength,
        expectedCrc,
        resume,
    };
}

export const encodeStartDraftPart = (request: StartDraftPartRequest): Uint8Array =>
    new Writer(64)
        .raw(request.childOperationId)
        .raw(request.parentOperationId)
        .u16(DRAFT_PART_KIND_CODE[request.draftPartKind])
        .zeros(2)
        .u64(request.partKey)
        .u64(request.declaredLength)
        .u32(request.expectedCrc)
        .u8(request.resume)
        .zeros(7)
        .finish();

export interface DraftPartAcceptance {
    readonly flags: number;
    readonly childOperationId: OperationId;
    readonly parentOperationId: OperationId;
    readonly sessionId: number;
    readonly draftPartKind: DraftPartKindName;
    readonly partKey: PartKey;
    readonly durableNextOffset: bigint;
    readonly checkpointGranule: number;
    readonly maximumStreamPayload: number;
    readonly finalizedPrefixCrc: number;
}

export function decodeDraftPartAccepted(bytes: Uint8Array): DispositionResponse<DraftPartAcceptance> {
    const cursor = new Cursor(bytes);
    const disposition = cursor.u8();
    if (disposition === 1) return { disposition: "alreadyTerminal", result: decodeTerminalDisposition(cursor, bytes) };
    if (disposition !== 0) reject("invalidDescriptor", "unknownEnum", `disposition ${disposition} is not registered`);
    cursor.zeros(1, "DraftPartAccepted byte 1");
    const flags = cursor.u16();
    const child = operationId(cursor.take(16));
    const parent = operationId(cursor.take(16));
    const session = nonzeroSession(cursor.u32());
    const kind = draftPartKindOf(cursor.u16());
    cursor.zeros(2, "DraftPartAccepted byte 42");
    const key = partKey(cursor.u64());
    const durableNextOffset = cursor.u64();
    const checkpointGranule = cursor.u32();
    const maximumStreamPayload = cursor.u16();
    cursor.zeros(2, "DraftPartAccepted byte 66");
    const finalizedPrefixCrc = cursor.u32();
    cursor.end("DraftPartAccepted");
    checkAcceptanceFlags(flags, durableNextOffset, finalizedPrefixCrc);
    return {
        disposition: "accepted",
        accepted: {
            flags,
            childOperationId: child,
            parentOperationId: parent,
            sessionId: session,
            draftPartKind: kind,
            partKey: key,
            durableNextOffset,
            checkpointGranule,
            maximumStreamPayload,
            finalizedPrefixCrc,
        },
    };
}

export function encodeDraftPartAccepted(response: DispositionResponse<DraftPartAcceptance>): Uint8Array {
    if (response.disposition === "alreadyTerminal") return encodeTerminalDisposition(response.result);
    const accepted = response.accepted;
    return new Writer(72)
        .u8(0)
        .zeros(1)
        .u16(accepted.flags)
        .raw(accepted.childOperationId)
        .raw(accepted.parentOperationId)
        .u32(accepted.sessionId)
        .u16(DRAFT_PART_KIND_CODE[accepted.draftPartKind])
        .zeros(2)
        .u64(accepted.partKey)
        .u64(accepted.durableNextOffset)
        .u32(accepted.checkpointGranule)
        .u16(accepted.maximumStreamPayload)
        .zeros(2)
        .u32(accepted.finalizedPrefixCrc)
        .finish();
}

export interface FinalizeDraftRequest {
    readonly parentOperationId: OperationId;
}

export function decodeFinalizeDraft(bytes: Uint8Array): FinalizeDraftRequest {
    const cursor = new Cursor(bytes);
    const parent = operationId(cursor.take(16));
    cursor.end("FinalizeDraft");
    return { parentOperationId: parent };
}

export const encodeFinalizeDraft = (request: FinalizeDraftRequest): Uint8Array =>
    new Writer(16).raw(request.parentOperationId).finish();

export interface FinalizeDraftAcceptance {
    readonly flags: number;
    readonly parentOperationId: OperationId;
    readonly sessionId: number;
    readonly logicalObjectId: LogicalObjectId;
    readonly repositoryRevision: Revision;
    readonly durableManifestOffset: bigint;
    readonly checkpointGranule: number;
    readonly maximumStreamPayload: number;
    readonly finalizedPrefixCrc: number;
}

export function decodeFinalizeDraftResponse(bytes: Uint8Array): DispositionResponse<FinalizeDraftAcceptance> {
    const cursor = new Cursor(bytes);
    const disposition = cursor.u8();
    if (disposition === 1) return { disposition: "alreadyTerminal", result: decodeTerminalDisposition(cursor, bytes) };
    if (disposition !== 0) reject("invalidDescriptor", "unknownEnum", `disposition ${disposition} is not registered`);
    cursor.zeros(1, "FinalizeDraft acceptance byte 1");
    const flags = cursor.u16();
    const parent = operationId(cursor.take(16));
    const session = nonzeroSession(cursor.u32());
    const logical = logicalObjectId(cursor.u64());
    const repositoryRevision = revision(cursor.u64());
    const durableManifestOffset = cursor.u64();
    const checkpointGranule = cursor.u32();
    const maximumStreamPayload = cursor.u16();
    cursor.zeros(2, "FinalizeDraft acceptance byte 54");
    const finalizedPrefixCrc = cursor.u32();
    cursor.zeros(4, "FinalizeDraft acceptance byte 60");
    cursor.end("FinalizeDraft acceptance");
    checkAcceptanceFlags(flags, durableManifestOffset, finalizedPrefixCrc);
    return {
        disposition: "accepted",
        accepted: {
            flags,
            parentOperationId: parent,
            sessionId: session,
            logicalObjectId: logical,
            repositoryRevision,
            durableManifestOffset,
            checkpointGranule,
            maximumStreamPayload,
            finalizedPrefixCrc,
        },
    };
}

export function encodeFinalizeDraftResponse(response: DispositionResponse<FinalizeDraftAcceptance>): Uint8Array {
    if (response.disposition === "alreadyTerminal") return encodeTerminalDisposition(response.result);
    const accepted = response.accepted;
    return new Writer(64)
        .u8(0)
        .zeros(1)
        .u16(accepted.flags)
        .raw(accepted.parentOperationId)
        .u32(accepted.sessionId)
        .u64(accepted.logicalObjectId)
        .u64(accepted.repositoryRevision)
        .u64(accepted.durableManifestOffset)
        .u32(accepted.checkpointGranule)
        .u16(accepted.maximumStreamPayload)
        .zeros(2)
        .u32(accepted.finalizedPrefixCrc)
        .zeros(4)
        .finish();
}

// ------------------------------------------------------------------------------- queries (§8)

export interface QueryOperationRequest {
    readonly operationId: OperationId;
}

export function decodeQueryOperation(bytes: Uint8Array): QueryOperationRequest {
    const cursor = new Cursor(bytes);
    const id = operationId(cursor.take(16));
    cursor.end("QueryOperation");
    return { operationId: id };
}

export const encodeQueryOperation = (request: QueryOperationRequest): Uint8Array =>
    new Writer(16).raw(request.operationId).finish();

export interface OperationProgress {
    readonly namespace: number;
    readonly phase: number;
    readonly flags: number;
    readonly subjectKind: number;
    readonly logicalObjectId: LogicalObjectId;
    readonly durableOffset: bigint;
}

export type QueryOperationResponse =
    | { readonly state: "unknown" }
    | { readonly state: "inProgress"; readonly progress: OperationProgress }
    | { readonly state: "committed"; readonly result: ResultEnvelope }
    | { readonly state: "aborted"; readonly error: ErrorBody };

export function decodeQueryOperationResponse(bytes: Uint8Array): QueryOperationResponse {
    const cursor = new Cursor(bytes);
    const state = cursor.u8();
    cursor.zeros(3, "QueryOperation state reserved bytes");
    const rest = bytes.subarray(cursor.position);
    switch (state) {
        case OPERATION_STATE.unknown:
            if (rest.length !== 0) reject("invalidFrame", "trailingBytes", "Unknown carries no further bytes");
            return { state: "unknown" };
        case OPERATION_STATE.inProgress:
            return { state: "inProgress", progress: decodeOperationProgress(rest) };
        case OPERATION_STATE.committed:
            return { state: "committed", result: decodeResultEnvelope(rest) };
        case OPERATION_STATE.aborted:
            return { state: "aborted", error: decodeErrorBody(rest) };
        default:
            return reject("invalidDescriptor", "unknownEnum", `operation state ${state} is not registered`);
    }
}

export function encodeQueryOperationResponse(response: QueryOperationResponse): Uint8Array {
    const writer = new Writer(80);
    switch (response.state) {
        case "unknown":
            return writer.u8(OPERATION_STATE.unknown).zeros(3).finish();
        case "inProgress":
            return writer.u8(OPERATION_STATE.inProgress).zeros(3).raw(encodeOperationProgress(response.progress)).finish();
        case "committed":
            return writer.u8(OPERATION_STATE.committed).zeros(3).raw(encodeResultEnvelope(response.result)).finish();
        case "aborted":
            return writer.u8(OPERATION_STATE.aborted).zeros(3).raw(encodeErrorBody(response.error)).finish();
    }
}

function decodeOperationProgress(bytes: Uint8Array): OperationProgress {
    const cursor = new Cursor(bytes);
    const namespace = cursor.u8();
    const phase = cursor.u8();
    const flags = cursor.u8();
    cursor.zeros(1, "progress byte 3");
    const subjectKind = cursor.u16();
    cursor.zeros(2, "progress byte 6");
    const logical = logicalObjectId(cursor.u64());
    const durableOffset = cursor.u64();
    cursor.end("operation progress");

    if (phase > MAX_PHASE) reject("invalidDescriptor", "unknownEnum", `phase ${phase} is not registered`);
    if ((flags & ~PROGRESS_FLAG_MASK) !== 0) reject("invalidDescriptor", "reservedBits", "progress bits 3..7 are zero");
    if (namespace === 0) {
        if (subjectKind !== 0) {
            reject("invalidDescriptor", "invalidCombination", "namespace none carries a zero subject kind");
        }
    } else if (namespace === 1) {
        objectKindOf(subjectKind);
    } else if (namespace === 2) {
        draftPartKindOf(subjectKind);
    } else {
        reject("invalidDescriptor", "unknownEnum", `subject namespace ${namespace} is not registered`);
    }
    if ((flags & PROGRESS_FLAG.logicalIdPresent) === 0 && logical !== 0n) {
        reject("invalidDescriptor", "reservedBits", "an ID field with logical-ID-present clear is zero");
    }
    return { namespace, phase, flags, subjectKind, logicalObjectId: logical, durableOffset };
}

const encodeOperationProgress = (progress: OperationProgress): Uint8Array =>
    new Writer(24)
        .u8(progress.namespace)
        .u8(progress.phase)
        .u8(progress.flags)
        .zeros(1)
        .u16(progress.subjectKind)
        .zeros(2)
        .u64(progress.logicalObjectId)
        .u64(progress.durableOffset)
        .finish();

export const CATALOG_FLAG = { expectedRevision: 1 << 0, cursor: 1 << 1 } as const;
const CATALOG_FLAG_MASK = 0x03;

export interface QueryCatalogRequest {
    readonly objectKind: ObjectKindName;
    readonly flags: number;
    readonly expectedRevision: Revision;
    readonly cursor: PageCursor;
}

/** §8.2 cursor codec. Opaque to application code despite being normative. */
export interface CatalogCursorFields {
    readonly repositoryRevision: bigint;
    readonly nextEntryIndex: number;
    readonly objectKind: number;
    readonly crc: number;
}

export function readCatalogCursor(cursor: Uint8Array): CatalogCursorFields {
    const reader = new Cursor(cursor);
    return {
        repositoryRevision: reader.u64(),
        nextEntryIndex: reader.u16(),
        objectKind: reader.u16(),
        crc: reader.u32(),
    };
}

export function decodeQueryCatalog(bytes: Uint8Array): QueryCatalogRequest {
    const reader = new Cursor(bytes);
    const objectKind = objectKindOf(reader.u16());
    const flags = reader.u16();
    const expected = revision(reader.u64());
    const cursorBytes = pageCursor(reader.take(16));
    reader.end("QueryCatalog");

    if ((flags & ~CATALOG_FLAG_MASK) !== 0) {
        reject("invalidDescriptor", "reservedBits", "QueryCatalog flags above bit 1 are zero");
    }
    const hasRevision = (flags & CATALOG_FLAG.expectedRevision) !== 0;
    const hasCursor = (flags & CATALOG_FLAG.cursor) !== 0;
    if (!hasRevision && expected !== 0n) {
        reject("invalidDescriptor", "reservedBits", "an inactive expected revision is encoded zero");
    }
    if (!hasCursor && !identityIsZero(cursorBytes)) {
        reject("invalidDescriptor", "reservedBits", "an inactive cursor is encoded zero");
    }
    if (hasCursor && !hasRevision) {
        reject("invalidDescriptor", "invalidCombination", "a cursor requires both flags and an expected revision");
    }
    if (hasCursor) {
        const fields = readCatalogCursor(cursorBytes);
        if (fields.repositoryRevision !== expected) {
            reject("invalidDescriptor", "invalidCombination", "the cursor revision and the expected revision disagree");
        }
        if (fields.objectKind !== OBJECT_KIND_CODE[objectKind]) {
            reject("invalidDescriptor", "invalidCombination", "the cursor names another ObjectKind");
        }
    }
    return { objectKind, flags, expectedRevision: expected, cursor: cursorBytes };
}

export const encodeQueryCatalog = (request: QueryCatalogRequest): Uint8Array =>
    new Writer(28)
        .u16(OBJECT_KIND_CODE[request.objectKind])
        .u16(request.flags)
        .u64(request.expectedRevision)
        .raw(request.cursor)
        .finish();

export interface CatalogEntry {
    readonly logicalObjectId: LogicalObjectId;
    readonly objectRevision: Revision;
    readonly length: bigint;
    readonly crc: number;
    readonly flags: number;
    readonly metadata: MetadataEnvelope;
}

export interface QueryCatalogResponse {
    readonly storeId: StoreId;
    readonly objectKind: ObjectKindName;
    readonly repositoryRevision: Revision;
    readonly nextCursor: PageCursor;
    readonly entries: readonly CatalogEntry[];
}

export const CATALOG_ENTRY_PREFIX_BYTES = 36;
export const CATALOG_PAGE_PREFIX_BYTES = 44;
export const MAX_CATALOG_ENTRIES_PER_PAGE = 10;

export function decodeQueryCatalogResponse(bytes: Uint8Array, more: boolean): QueryCatalogResponse {
    const reader = new Cursor(bytes);
    const store = storeId(reader.take(16));
    const objectKind = objectKindOf(reader.u16());
    const entryCount = reader.u16();
    const repositoryRevision = revision(reader.u64());
    const nextCursor = pageCursor(reader.take(16));

    if (!more && !identityIsZero(nextCursor)) {
        reject("invalidDescriptor", "reservedBits", "the next cursor is zero unless `more` is set");
    }
    if (entryCount > MAX_CATALOG_ENTRIES_PER_PAGE) {
        reject("invalidDescriptor", "invalidCombination", `a page returns at most ${MAX_CATALOG_ENTRIES_PER_PAGE} entries`);
    }

    const entries: CatalogEntry[] = [];
    let previousId: bigint | undefined;
    for (let i = 0; i < entryCount; i++) {
        const logical = logicalObjectId(reader.u64());
        const objectRevision = revision(reader.u64());
        const length = reader.u64();
        const crc = reader.u32();
        const flags = reader.u16();
        const metadataLength = reader.u16();
        reader.zeros(4, "catalog entry byte 32");
        if (flags !== 0) reject("invalidDescriptor", "reservedBits", "catalog entry flags are zero in v3.0");
        const metadataBytes = reader.take(metadataLength);
        const metadata = decodeMetadataEnvelope(metadataBytes, { kind: objectKind, role: "catalog", mutating: false });
        if (metadata.byteLength !== metadataLength) {
            reject("invalidDescriptor", "nestedLength", "the entry metadata length disagrees with the envelope");
        }
        if (previousId !== undefined && logical <= previousId) {
            reject("invalidDescriptor", "invalidCombination", "entries are ordered by LogicalObjectId");
        }
        previousId = logical;
        entries.push({ logicalObjectId: logical, objectRevision, length, crc, flags, metadata });
    }
    reader.end("catalog page");
    return { storeId: store, objectKind, repositoryRevision, nextCursor, entries };
}

export function encodeQueryCatalogResponse(response: QueryCatalogResponse): Uint8Array {
    const writer = new Writer(CATALOG_PAGE_PREFIX_BYTES + 128);
    writer
        .raw(response.storeId)
        .u16(OBJECT_KIND_CODE[response.objectKind])
        .u16(response.entries.length)
        .u64(response.repositoryRevision)
        .raw(response.nextCursor);
    for (const entry of response.entries) {
        const metadata = encodeMetadataEnvelope(entry.metadata);
        writer
            .u64(entry.logicalObjectId)
            .u64(entry.objectRevision)
            .u64(entry.length)
            .u32(entry.crc)
            .u16(entry.flags)
            .u16(metadata.length)
            .zeros(4)
            .raw(metadata);
    }
    return writer.finish();
}

export const DRAFT_FLAG = { expectedRevision: 1 << 0, cursor: 1 << 1 } as const;
const DRAFT_FLAG_MASK = 0x03;
export const MAX_DRAFT_PAGE_ENTRIES = 6;
export const DRAFT_ENTRY_BYTES = 68;

export interface QueryDraftRequest {
    readonly parentOperationId: OperationId;
    readonly flags: number;
    readonly limit: number;
    readonly expectedDraftRevision: DraftRevision;
    readonly cursor: PageCursor;
}

export function decodeQueryDraft(bytes: Uint8Array): QueryDraftRequest {
    const reader = new Cursor(bytes);
    const parent = operationId(reader.take(16));
    const flags = reader.u16();
    const limit = reader.u8();
    reader.zeros(1, "QueryDraft byte 19");
    const expected = makeDraftRevision(reader.u64());
    const cursorBytes = pageCursor(reader.take(16));
    reader.end("QueryDraft");

    if ((flags & ~DRAFT_FLAG_MASK) !== 0) {
        reject("invalidDescriptor", "reservedBits", "QueryDraft flags above bit 1 are zero");
    }
    if (limit < 1 || limit > MAX_DRAFT_PAGE_ENTRIES) {
        reject("invalidDescriptor", "invalidCombination", `the requested limit is 1 through ${MAX_DRAFT_PAGE_ENTRIES}`);
    }
    const hasRevision = (flags & DRAFT_FLAG.expectedRevision) !== 0;
    const hasCursor = (flags & DRAFT_FLAG.cursor) !== 0;
    if (!hasRevision && expected !== 0n) {
        reject("invalidDescriptor", "reservedBits", "an inactive expected draft revision is encoded zero");
    }
    if (!hasCursor && !identityIsZero(cursorBytes)) {
        reject("invalidDescriptor", "reservedBits", "an inactive cursor is encoded zero");
    }
    if (hasCursor && !hasRevision) {
        reject("invalidDescriptor", "invalidCombination", "a cursor requires both flags and an expected revision");
    }
    if (hasCursor) {
        const fields = readCatalogCursor(cursorBytes);
        if (fields.repositoryRevision !== expected) {
            reject("invalidDescriptor", "invalidCombination", "the cursor revision and the expected revision disagree");
        }
        if (fields.objectKind !== 0) {
            reject("invalidDescriptor", "reservedBits", "the draft cursor's third field is zero");
        }
    }
    return { parentOperationId: parent, flags, limit, expectedDraftRevision: expected, cursor: cursorBytes };
}

export const encodeQueryDraft = (request: QueryDraftRequest): Uint8Array =>
    new Writer(44)
        .raw(request.parentOperationId)
        .u16(request.flags)
        .u8(request.limit)
        .zeros(1)
        .u64(request.expectedDraftRevision)
        .raw(request.cursor)
        .finish();

export interface DraftEntry {
    readonly childOperationId: OperationId;
    readonly draftPartRef: DraftPartRef;
    readonly draftPartKind: DraftPartKindName;
    readonly partKey: PartKey;
    readonly state: number;
    readonly durableOffset: bigint;
    readonly declaredLength: bigint;
    readonly crc: number;
}

export const DRAFT_PAGE_FLAG = { manifestStreaming: 1 << 0, aborting: 1 << 1 } as const;

export interface QueryDraftResponse {
    readonly parentOperationId: OperationId;
    readonly draftRevision: DraftRevision;
    readonly nextCursor: PageCursor;
    readonly flags: number;
    readonly entries: readonly DraftEntry[];
}

export function decodeQueryDraftResponse(bytes: Uint8Array, more: boolean): QueryDraftResponse {
    const reader = new Cursor(bytes);
    const parent = operationId(reader.take(16));
    const revisionValue = makeDraftRevision(reader.u64());
    const nextCursor = pageCursor(reader.take(16));
    const count = reader.u8();
    const flags = reader.u8();
    reader.zeros(2, "draft page reserved bytes");

    if (!more && !identityIsZero(nextCursor)) {
        reject("invalidDescriptor", "reservedBits", "the next cursor is zero unless `more` is set");
    }
    if ((flags & ~0x03) !== 0) reject("invalidDescriptor", "reservedBits", "draft page flags above bit 1 are zero");
    if (count > MAX_DRAFT_PAGE_ENTRIES) {
        reject("invalidDescriptor", "invalidCombination", `a draft page carries at most ${MAX_DRAFT_PAGE_ENTRIES} entries`);
    }

    const entries: DraftEntry[] = [];
    let previous: { kind: number; key: bigint } | undefined;
    for (let i = 0; i < count; i++) {
        const child = operationId(reader.take(16));
        const ref = draftPartRef(reader.take(16));
        const kindCode = reader.u16();
        const kind = draftPartKindOf(kindCode);
        reader.zeros(2, "draft entry byte 34");
        const key = partKey(reader.u64());
        const state = cursorState(reader.u8());
        reader.zeros(1, "draft entry flags byte");
        reader.zeros(2, "draft entry byte 54");
        const durableOffset = reader.u64();
        const declaredLength = reader.u64();
        const crc = reader.u32();
        if (state !== DRAFT_PART_STATE.sealed && !identityIsZero(ref)) {
            reject("invalidDescriptor", "invalidCombination", "a DraftPartRef is zero unless the state is sealed");
        }
        if (previous !== undefined && (kindCode < previous.kind || (kindCode === previous.kind && key <= previous.key))) {
            reject("invalidDescriptor", "invalidCombination", "entries are strictly ordered by (DraftPartKind, part key)");
        }
        previous = { kind: kindCode, key };
        entries.push({
            childOperationId: child,
            draftPartRef: ref,
            draftPartKind: kind,
            partKey: key,
            state,
            durableOffset,
            declaredLength,
            crc,
        });
    }
    reader.end("draft page");
    return { parentOperationId: parent, draftRevision: revisionValue, nextCursor, flags, entries };
}

const cursorState = (state: number): number => enumIn(state, [0, 1, 2, 3], "draft part state");

export function encodeQueryDraftResponse(response: QueryDraftResponse): Uint8Array {
    const writer = new Writer(44 + response.entries.length * DRAFT_ENTRY_BYTES);
    writer
        .raw(response.parentOperationId)
        .u64(response.draftRevision)
        .raw(response.nextCursor)
        .u8(response.entries.length)
        .u8(response.flags)
        .zeros(2);
    for (const entry of response.entries) {
        writer
            .raw(entry.childOperationId)
            .raw(entry.draftPartRef)
            .u16(DRAFT_PART_KIND_CODE[entry.draftPartKind])
            .zeros(2)
            .u64(entry.partKey)
            .u8(entry.state)
            .zeros(1)
            .zeros(2)
            .u64(entry.durableOffset)
            .u64(entry.declaredLength)
            .u32(entry.crc);
    }
    return writer.finish();
}

export const WEATHER_FLAG = { headPresent: 1 << 0 } as const;
export const WEATHER_CONTEXT_STATE = { pending: 1, satisfied: 2 } as const;

export interface WeatherRequestContext {
    readonly storeId: StoreId;
    readonly currentWeatherRequestId: WeatherRequestId;
    readonly requestContextRevision: Revision;
    readonly flags: number;
    readonly weatherLogicalObjectId: LogicalObjectId;
    readonly weatherRepositoryRevision: Revision;
    readonly headWeatherRequestId: WeatherRequestId;
    readonly centreLatitude: number;
    readonly centreLongitude: number;
    readonly radiusMetres: number;
    readonly earliestIssuedUtc: bigint;
    readonly requiredValidUntilUtc: bigint;
    readonly contextState: number;
}

export function decodeWeatherRequestContext(bytes: Uint8Array): WeatherRequestContext {
    const reader = new Cursor(bytes);
    const store = storeId(reader.take(16));
    const currentWeatherRequestId = weatherRequestId(reader.u64());
    const requestContextRevision = revision(reader.u64());
    const flags = reader.u32();
    const weatherLogicalObjectId = logicalObjectId(reader.u64());
    const weatherRepositoryRevision = revision(reader.u64());
    const headWeatherRequestId = weatherRequestId(reader.u64());
    const centreLatitude = reader.i32();
    const centreLongitude = reader.i32();
    const radiusMetres = reader.u32();
    const earliestIssuedUtc = reader.i64();
    const requiredValidUntilUtc = reader.i64();
    const contextState = enumIn(reader.u8(), [1, 2], "weather context state");
    reader.zeros(7, "weather context reserved bytes");
    reader.end("weather request context");
    if ((flags & ~0x01) !== 0) reject("invalidDescriptor", "reservedBits", "weather flags above bit 0 are zero");
    if ((flags & WEATHER_FLAG.headPresent) === 0 && headWeatherRequestId !== 0n) {
        reject("invalidDescriptor", "reservedBits", "the head WeatherRequestId is zero when head-present is clear");
    }
    return {
        storeId: store,
        currentWeatherRequestId,
        requestContextRevision,
        flags,
        weatherLogicalObjectId,
        weatherRepositoryRevision,
        headWeatherRequestId,
        centreLatitude,
        centreLongitude,
        radiusMetres,
        earliestIssuedUtc,
        requiredValidUntilUtc,
        contextState,
    };
}

export const encodeWeatherRequestContext = (context: WeatherRequestContext): Uint8Array =>
    new Writer(96)
        .raw(context.storeId)
        .u64(context.currentWeatherRequestId)
        .u64(context.requestContextRevision)
        .u32(context.flags)
        .u64(context.weatherLogicalObjectId)
        .u64(context.weatherRepositoryRevision)
        .u64(context.headWeatherRequestId)
        .i32(context.centreLatitude)
        .i32(context.centreLongitude)
        .u32(context.radiusMetres)
        .i64(context.earliestIssuedUtc)
        .i64(context.requiredValidUntilUtc)
        .u8(context.contextState)
        .zeros(7)
        .finish();

// --------------------------------------------------------------------------- direct mutations (§9)

export const MUTATION_FLAG = { expectedRevision: 1 << 0 } as const;

export interface MutationTarget {
    readonly operationId: OperationId;
    readonly objectKind: ObjectKindName;
    readonly flags: number;
    readonly logicalObjectId: LogicalObjectId;
    readonly expectedRevision: Revision;
}

function readMutationTarget(reader: Cursor): MutationTarget {
    const id = operationId(reader.take(16));
    const objectKind = objectKindOf(reader.u16());
    const flags = reader.u16();
    const logical = logicalObjectId(reader.u64());
    const expected = revision(reader.u64());
    if ((flags & ~MUTATION_FLAG.expectedRevision) !== 0) {
        reject("invalidDescriptor", "reservedBits", "mutation flags above bit 0 are zero");
    }
    if ((flags & MUTATION_FLAG.expectedRevision) === 0) {
        reject("invalidDescriptor", "invalidCombination", "the expected-revision flag is mandatory");
    }
    return { operationId: id, objectKind, flags, logicalObjectId: logical, expectedRevision: expected };
}

function writeMutationTarget(writer: Writer, target: MutationTarget): Writer {
    return writer
        .raw(target.operationId)
        .u16(OBJECT_KIND_CODE[target.objectKind])
        .u16(target.flags)
        .u64(target.logicalObjectId)
        .u64(target.expectedRevision);
}

export function decodeDeleteObject(bytes: Uint8Array): MutationTarget {
    const reader = new Cursor(bytes);
    const target = readMutationTarget(reader);
    reader.end("DeleteObject");
    return target;
}

export const encodeDeleteObject = (target: MutationTarget): Uint8Array =>
    writeMutationTarget(new Writer(36), target).finish();

export interface SetMetadataRequest extends MutationTarget {
    readonly metadata: MetadataEnvelope;
}

export function decodeSetMetadata(bytes: Uint8Array): SetMetadataRequest {
    const reader = new Cursor(bytes);
    const target = readMutationTarget(reader);
    const metadata = decodeMetadataEnvelope(bytes.subarray(reader.position), {
        kind: target.objectKind,
        role: "patch",
        mutating: true,
    });
    if (bytes.length !== reader.position + metadata.byteLength) {
        reject("invalidFrame", "trailingBytes", "SetMetadata ends with exactly one metadata envelope");
    }
    if (metadata.fields.length === 0) {
        reject(
            "invalidDescriptor",
            "emptyMetadataPatch",
            "a mutation that changes nothing would still consume an OperationId, a claim, and a catalog commit",
        );
    }
    return { ...target, metadata };
}

export const encodeSetMetadata = (request: SetMetadataRequest): Uint8Array =>
    writeMutationTarget(new Writer(176), request).raw(encodeMetadataEnvelope(request.metadata)).finish();

export interface OperationOnObject {
    readonly operationId: OperationId;
    readonly logicalObjectId: LogicalObjectId;
    readonly expectedRevision: Revision;
}

export function decodeOperationOnObject(bytes: Uint8Array, what: string): OperationOnObject {
    const reader = new Cursor(bytes);
    const id = operationId(reader.take(16));
    const logical = logicalObjectId(reader.u64());
    const expected = revision(reader.u64());
    reader.end(what);
    return { operationId: id, logicalObjectId: logical, expectedRevision: expected };
}

export const encodeOperationOnObject = (request: OperationOnObject): Uint8Array =>
    new Writer(32).raw(request.operationId).u64(request.logicalObjectId).u64(request.expectedRevision).finish();

// ------------------------------------------------------------------- device-control plane (§16)

export interface DeviceStatus {
    readonly firmwareMajor: number;
    readonly firmwareMinor: number;
    readonly firmwarePatch: number;
    readonly hardwareRevision: number;
    readonly deviceSerial: DeviceSerial;
    readonly bootCount: number;
    readonly uptimeSeconds: bigint;
    readonly stackHighWaterBytes: number;
    readonly statusFlags: number;
    readonly mountClass: number;
    readonly firmwareBuildNumber: number;
    readonly storeId: StoreId;
}

export const DEVICE_STATUS_FLAG = { cardPresent: 1 << 0, developerUnlocked: 1 << 1 } as const;

export function decodeDeviceStatus(bytes: Uint8Array): DeviceStatus {
    const reader = new Cursor(bytes);
    const firmwareMajor = reader.u16();
    const firmwareMinor = reader.u16();
    const firmwarePatch = reader.u16();
    const hardwareRevision = reader.u16();
    const serial = deviceSerial(reader.take(16));
    const bootCount = reader.u32();
    const uptimeSeconds = reader.u64();
    const stackHighWaterBytes = reader.u32();
    const statusFlags = reader.u16();
    const mountClass = reader.u8();
    reader.zeros(1, "device status byte 43");
    const firmwareBuildNumber = reader.u32();
    const store = storeId(reader.take(16));
    reader.end("device status");

    if ((statusFlags & ~0x03) !== 0) reject("invalidDescriptor", "reservedBits", "status flags above bit 1 are zero");
    if (mountClass > MAX_MOUNT_CLASS) {
        reject("invalidDescriptor", "unknownEnum", `mount class ${mountClass} is not registered`);
    }
    if (!CLASSES_REPORTING_A_STORE.includes(mountClass) && !identityIsZero(store)) {
        reject("invalidDescriptor", "reservedBits", "the StoreId is zero unless the mount class is 3, 4, or 6");
    }
    return {
        firmwareMajor,
        firmwareMinor,
        firmwarePatch,
        hardwareRevision,
        deviceSerial: serial,
        bootCount,
        uptimeSeconds,
        stackHighWaterBytes,
        statusFlags,
        mountClass,
        firmwareBuildNumber,
        storeId: store,
    };
}

export const encodeDeviceStatus = (status: DeviceStatus): Uint8Array =>
    new Writer(64)
        .u16(status.firmwareMajor)
        .u16(status.firmwareMinor)
        .u16(status.firmwarePatch)
        .u16(status.hardwareRevision)
        .raw(status.deviceSerial)
        .u32(status.bootCount)
        .u64(status.uptimeSeconds)
        .u32(status.stackHighWaterBytes)
        .u16(status.statusFlags)
        .u8(status.mountClass)
        .zeros(1)
        .u32(status.firmwareBuildNumber)
        .raw(status.storeId)
        .finish();

export const UNIT_FLAG = { imperial: 1 << 0, fahrenheit: 1 << 1, twelveHourClock: 1 << 2 } as const;

export interface ConfigBlock {
    readonly codecVersion: number;
    readonly blockLength: number;
    readonly unitFlags: number;
    readonly weatherRefresh: number;
    readonly deviceNameLength: number;
    readonly deviceName: string;
}

export function decodeConfigBlock(bytes: Uint8Array): ConfigBlock {
    const reader = new Cursor(bytes);
    const codecVersion = reader.u8();
    const blockLength = reader.u8();
    const flags = reader.u16();
    const deviceNameLength = reader.u8();
    const unitFlags = reader.u8();
    const weatherRefresh = reader.u8();
    reader.zeros(1, "config byte 7");
    const nameField = reader.take(MAX_DEVICE_NAME_BYTES);
    reader.zeros(16, "config bytes 40..55");
    reader.end("config block");

    if (codecVersion !== CONFIG_CODEC_VERSION) {
        reject("invalidDescriptor", "invalidCombination", `config codec version ${codecVersion} is unknown`);
    }
    if (blockLength !== CONFIG_BLOCK_BYTES) {
        reject("invalidDescriptor", "invalidCombination", `the config block length is ${CONFIG_BLOCK_BYTES}`);
    }
    if (flags !== 0) reject("invalidDescriptor", "reservedBits", "config flags are zero");
    if (deviceNameLength > MAX_DEVICE_NAME_BYTES) {
        reject("invalidDescriptor", "invalidCombination", `the device-name length is 0 through ${MAX_DEVICE_NAME_BYTES}`);
    }
    if ((unitFlags & ~UNIT_FLAG_MASK) !== 0) {
        reject("invalidDescriptor", "unsupportedFlags", "unit flags above bit 2 are zero");
    }
    if (weatherRefresh > MAX_WEATHER_REFRESH) {
        reject("invalidDescriptor", "unknownEnum", `weather refresh ${weatherRefresh} is not registered`);
    }
    for (let i = deviceNameLength; i < MAX_DEVICE_NAME_BYTES; i++) {
        if (nameField[i] !== 0) {
            reject("invalidDescriptor", "reservedBits", "the name field is zero padded beyond its stated length");
        }
    }
    const deviceName = deviceNameLength === 0 ? "" : decodeWireText(nameField.subarray(0, deviceNameLength), "device name");
    return { codecVersion, blockLength, unitFlags, weatherRefresh, deviceNameLength, deviceName };
}

export function encodeConfigBlock(config: ConfigBlock): Uint8Array {
    const name = new Uint8Array(MAX_DEVICE_NAME_BYTES);
    name.set(new TextEncoder().encode(config.deviceName).subarray(0, MAX_DEVICE_NAME_BYTES));
    return new Writer(CONFIG_BLOCK_BYTES)
        .u8(config.codecVersion)
        .u8(config.blockLength)
        .u16(0)
        .u8(config.deviceNameLength)
        .u8(config.unitFlags)
        .u8(config.weatherRefresh)
        .zeros(1)
        .raw(name)
        .zeros(16)
        .finish();
}

export interface SetClockRequest {
    readonly epochSeconds: bigint;
    readonly source: number;
}

export function decodeSetClock(bytes: Uint8Array): SetClockRequest {
    const reader = new Cursor(bytes);
    const epochSeconds = reader.i64();
    const source = enumIn(reader.u8(), [CLOCK_SOURCE.companion, CLOCK_SOURCE.gps], "clock source");
    reader.zeros(7, "SetClock reserved bytes");
    reader.end("SetClock");
    return { epochSeconds, source };
}

export const encodeSetClock = (request: SetClockRequest): Uint8Array =>
    new Writer(16).i64(request.epochSeconds).u8(request.source).zeros(7).finish();

export interface ClockStatus {
    readonly epochSeconds: bigint;
    readonly source: number;
    readonly clockState: number;
}

export function decodeClockStatus(bytes: Uint8Array): ClockStatus {
    const reader = new Cursor(bytes);
    const epochSeconds = reader.i64();
    const source = enumIn(reader.u8(), [0, CLOCK_SOURCE.companion, CLOCK_SOURCE.gps], "clock source");
    const clockState = enumIn(reader.u8(), [CLOCK_STATE.untrusted, CLOCK_STATE.trusted], "clock state");
    reader.zeros(6, "SetClock response reserved bytes");
    reader.end("SetClock response");
    return { epochSeconds, source, clockState };
}

export const encodeClockStatus = (status: ClockStatus): Uint8Array =>
    new Writer(16).i64(status.epochSeconds).u8(status.source).u8(status.clockState).zeros(6).finish();

export interface ForgetBondRequest {
    readonly scope: number;
}

export function decodeForgetBond(bytes: Uint8Array): ForgetBondRequest {
    const reader = new Cursor(bytes);
    const scope = enumIn(reader.u8(), [FORGET_BOND_SCOPE.thisBond, FORGET_BOND_SCOPE.everyBond], "ForgetBond scope");
    reader.zeros(7, "ForgetBond reserved bytes");
    reader.end("ForgetBond");
    return { scope };
}

export const encodeForgetBond = (request: ForgetBondRequest): Uint8Array =>
    new Writer(8).u8(request.scope).zeros(7).finish();

export interface StoreIdMessage {
    readonly storeId: StoreId;
}

export function decodeStoreIdMessage(bytes: Uint8Array, what: string): StoreIdMessage {
    const reader = new Cursor(bytes);
    const store = storeId(reader.take(16));
    reader.end(what);
    return { storeId: store };
}

export const encodeStoreIdMessage = (message: StoreIdMessage): Uint8Array => new Writer(16).raw(message.storeId).finish();

/**
 * §16's ResetStore admission check, which needs a fact the frame does not carry: the mount class the
 * device currently reports. "The echo is the confirmation, and it is checked before anything is
 * deleted."
 */
export function validateResetStoreEcho(echo: Uint8Array, mountClass: number, currentStoreId?: Uint8Array): void {
    if (mountClass === MOUNT_CLASS.noCard) {
        reject("mediaUnavailable", "noCard", "ResetStore is the one device-control member that needs the medium");
    }
    if (mountClass === MOUNT_CLASS.unsupportedFilesystem) {
        reject("mediaUnavailable", "unmounted", "the device never formats a volume whose filesystem it does not accept");
    }
    if (!CLASSES_REPORTING_A_STORE.includes(mountClass)) {
        if (!identityIsZero(echo)) {
            reject(
                "invalidDescriptor",
                "invalidCombination",
                "a class that reports no StoreId is echoed all-zero",
            );
        }
        return;
    }
    if (identityIsZero(echo)) {
        reject(
            "invalidDescriptor",
            "invalidCombination",
            "an all-zero echo is admitted only in a class that reports no StoreId",
        );
    }
    if (currentStoreId !== undefined && !identityEquals(echo, currentStoreId)) {
        reject("invalidDescriptor", "invalidCombination", "the echoed StoreId must equal the one the device reports");
    }
}
