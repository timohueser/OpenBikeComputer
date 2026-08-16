/**
 * The control frame of Device_Object_Protocol_v3.md §2 and the operation registry of §4.
 *
 * The header alone decides three things a body decoder then relies on: which opcode's layout
 * applies, whether this is the request or the response side (the response flag, since the two share
 * an opcode), and whether it is an error (in which case the body is a §12 ErrorBody whatever the
 * opcode is).
 *
 * The rejection categories are deliberately distinct, and §2 spells out the split: `invalidFrame`
 * means a record cannot be established as one complete frame — bad length, truncation, trailing
 * bytes, bad magic, a payload-length mismatch — while `invalidDescriptor` means a complete frame
 * has an illegal field value. An unsupported but parseable wire version is `incompatibleVersion`
 * and neither of the malformed categories.
 */

import { Cursor, Writer } from "./bytes";
import {
    MAX_CONTROL_PAYLOAD,
    readCapabilities,
    readHello,
    writeCapabilities,
    writeHello,
    type Capabilities,
    type HelloRequest,
} from "./capabilities";
import { readErrorBody, writeErrorBody, type ErrorBody } from "./errorBody";
import { requestId as makeRequestId, type RequestId } from "./ids";
import {
    readAbortOperation,
    readAbortSession,
    readAbortSessionResponse,
    readBeginDraft,
    readBeginDraftResponse,
    readCheckpointResponse,
    readCheckpointUpload,
    readClockStatus,
    readConfigBlock,
    readDeleteObject,
    readDeviceStatus,
    readDownloadAccepted,
    readDraftPartAccepted,
    readFinalizeDraft,
    readFinalizeDraftResponse,
    readFinishDownload,
    readForgetBond,
    readOperationOnObject,
    readQueryCatalog,
    readQueryCatalogResponse,
    readQueryDraft,
    readQueryDraftResponse,
    readQueryOperation,
    readQueryOperationResponse,
    readResultEnvelope,
    readSessionOnly,
    readSetClock,
    readSetMetadata,
    readStartDownload,
    readStartDraftPart,
    readStartUpload,
    readStoreIdMessage,
    readUploadAccepted,
    readWeatherRequestContext,
    writeAbortOperation,
    writeAbortSession,
    writeAbortSessionResponse,
    writeBeginDraft,
    writeBeginDraftResponse,
    writeCheckpointResponse,
    writeCheckpointUpload,
    writeClockStatus,
    writeConfigBlock,
    writeDeleteObject,
    writeDeviceStatus,
    writeDownloadAccepted,
    writeDraftPartAccepted,
    writeFinalizeDraft,
    writeFinalizeDraftResponse,
    writeFinishDownload,
    writeForgetBond,
    writeOperationOnObject,
    writeQueryCatalog,
    writeQueryCatalogResponse,
    writeQueryDraft,
    writeQueryDraftResponse,
    writeQueryOperation,
    writeQueryOperationResponse,
    writeResultEnvelope,
    writeSessionOnly,
    writeSetClock,
    writeSetMetadata,
    writeStartDownload,
    writeStartDraftPart,
    writeStartUpload,
    writeStoreIdMessage,
    writeUploadAccepted,
    writeWeatherRequestContext,
    type AbortOperationRequest,
    type AbortSessionRequest,
    type AbortSessionResponse,
    type BodyContext,
    type BeginDraftAcceptance,
    type BeginDraftRequest,
    type CheckpointUploadRequest,
    type CheckpointUploadResponse,
    type ClockStatus,
    type ConfigBlock,
    type DeviceStatus,
    type DispositionResponse,
    type DownloadAccepted,
    type DraftPartAcceptance,
    type FinalizeDraftAcceptance,
    type FinalizeDraftRequest,
    type FinishDownloadRequest,
    type ForgetBondRequest,
    type MutationTarget,
    type OperationOnObject,
    type QueryCatalogRequest,
    type QueryCatalogResponse,
    type QueryDraftRequest,
    type QueryDraftResponse,
    type QueryOperationRequest,
    type QueryOperationResponse,
    type ResultEnvelope,
    type SessionOnlyRequest,
    type SetClockRequest,
    type SetMetadataRequest,
    type StartDownloadRequest,
    type StartDraftPartRequest,
    type StartUploadRequest,
    type StoreIdMessage,
    type UploadAcceptance,
    type WeatherRequestContext,
} from "./messages";
import { decoding, reject, type DosResult } from "./result";

export const CONTROL_MAGIC_BYTES = Uint8Array.of(0x4f, 0x42, 0x43, 0x50);
export const WIRE_MAJOR = 3;
export const WIRE_MINOR = 0;
export const CONTROL_HEADER_BYTES = 16;

/** §4's operation registry. Requests and successful responses share an opcode. */
export const OPCODE = {
    Hello: 0x0001,
    StartUpload: 0x0100,
    CheckpointUpload: 0x0101,
    FinishUpload: 0x0102,
    StartDownload: 0x0110,
    FinishDownload: 0x0111,
    AbortSession: 0x0120,
    BeginDraft: 0x0130,
    StartDraftPart: 0x0131,
    FinalizeDraft: 0x0132,
    QueryOperation: 0x0200,
    QueryCatalog: 0x0201,
    QueryDraft: 0x0202,
    QueryWeatherRequest: 0x0203,
    DeleteObject: 0x0300,
    SetMetadata: 0x0301,
    AbortOperation: 0x0302,
    InstallUpdate: 0x0310,
    AcknowledgeRideImported: 0x0311,
    GetDeviceStatus: 0x0400,
    GetConfig: 0x0401,
    SetConfig: 0x0402,
    SetClock: 0x0403,
    ForgetBond: 0x0404,
    Echo: 0x0405,
    ResetStore: 0x0406,
} as const;

export type OpcodeName = keyof typeof OPCODE;
export type Opcode = (typeof OPCODE)[OpcodeName];

export const OPCODE_NAME: ReadonlyMap<number, OpcodeName> = new Map(
    (Object.entries(OPCODE) as [OpcodeName, Opcode][]).map(([name, value]) => [value, name]),
);

/** §2: `more` is valid only on a paged Capabilities, QueryCatalog, or QueryDraft response. */
const PAGEABLE: readonly number[] = [OPCODE.Hello, OPCODE.QueryCatalog, OPCODE.QueryDraft];

export const HEADER_FLAG = { response: 1 << 0, error: 1 << 1, more: 1 << 2 } as const;
const HEADER_FLAG_MASK = 0x07;

export type ControlBody =
    | { readonly kind: "Hello"; readonly hello: HelloRequest }
    | { readonly kind: "Capabilities"; readonly capabilities: Capabilities }
    | { readonly kind: "StartUpload"; readonly request: StartUploadRequest }
    | { readonly kind: "UploadAccepted"; readonly response: DispositionResponse<UploadAcceptance> }
    | { readonly kind: "CheckpointUpload"; readonly request: CheckpointUploadRequest }
    | { readonly kind: "CheckpointAccepted"; readonly response: CheckpointUploadResponse }
    | { readonly kind: "FinishUpload"; readonly request: SessionOnlyRequest }
    | { readonly kind: "StartDownload"; readonly request: StartDownloadRequest }
    | { readonly kind: "DownloadAccepted"; readonly response: DownloadAccepted }
    | { readonly kind: "FinishDownload"; readonly request: FinishDownloadRequest }
    | { readonly kind: "AbortSession"; readonly request: AbortSessionRequest }
    | { readonly kind: "AbortSessionResult"; readonly response: AbortSessionResponse }
    | { readonly kind: "BeginDraft"; readonly request: BeginDraftRequest }
    | { readonly kind: "BeginDraftAccepted"; readonly response: DispositionResponse<BeginDraftAcceptance> }
    | { readonly kind: "StartDraftPart"; readonly request: StartDraftPartRequest }
    | { readonly kind: "DraftPartAccepted"; readonly response: DispositionResponse<DraftPartAcceptance> }
    | { readonly kind: "FinalizeDraft"; readonly request: FinalizeDraftRequest }
    | { readonly kind: "FinalizeDraftAccepted"; readonly response: DispositionResponse<FinalizeDraftAcceptance> }
    | { readonly kind: "QueryOperation"; readonly request: QueryOperationRequest }
    | { readonly kind: "OperationStatus"; readonly response: QueryOperationResponse }
    | { readonly kind: "QueryCatalog"; readonly request: QueryCatalogRequest }
    | { readonly kind: "CatalogPage"; readonly response: QueryCatalogResponse }
    | { readonly kind: "QueryDraft"; readonly request: QueryDraftRequest }
    | { readonly kind: "DraftPage"; readonly response: QueryDraftResponse }
    | { readonly kind: "QueryWeatherRequest" }
    | { readonly kind: "WeatherRequestContext"; readonly response: WeatherRequestContext }
    | { readonly kind: "DeleteObject"; readonly request: MutationTarget }
    | { readonly kind: "SetMetadata"; readonly request: SetMetadataRequest }
    | { readonly kind: "AbortOperation"; readonly request: AbortOperationRequest }
    | { readonly kind: "OperationOnObject"; readonly request: OperationOnObject }
    | { readonly kind: "TerminalResult"; readonly result: ResultEnvelope }
    | { readonly kind: "GetDeviceStatus" }
    | { readonly kind: "DeviceStatus"; readonly response: DeviceStatus }
    | { readonly kind: "GetConfig" }
    | { readonly kind: "ConfigBlock"; readonly config: ConfigBlock }
    | { readonly kind: "SetClock"; readonly request: SetClockRequest }
    | { readonly kind: "ClockStatus"; readonly response: ClockStatus }
    | { readonly kind: "ForgetBond"; readonly request: ForgetBondRequest }
    | { readonly kind: "Empty" }
    | { readonly kind: "Echo"; readonly payload: Uint8Array }
    | { readonly kind: "ResetStore"; readonly request: StoreIdMessage }
    | { readonly kind: "ResetStoreResult"; readonly response: StoreIdMessage }
    | { readonly kind: "Error"; readonly error: ErrorBody };

export interface ControlFrame {
    readonly opcode: Opcode;
    readonly opcodeName: OpcodeName;
    readonly requestId: RequestId;
    readonly response: boolean;
    readonly error: boolean;
    readonly more: boolean;
    readonly body: ControlBody;
}

export interface ControlDecodeOptions extends BodyContext {
    /** The negotiated control frame maximum, when one has been established (§1). */
    readonly maximumFrameBytes?: number;
}

/** Total decode: every malformed record becomes a typed §12 category rather than an exception. */
export const decodeControlFrame = (bytes: Uint8Array, options: ControlDecodeOptions = {}): DosResult<ControlFrame> =>
    decoding(() => readControlFrame(bytes, options));

function readControlFrame(bytes: Uint8Array, options: ControlDecodeOptions): ControlFrame {
    if (bytes.length < CONTROL_HEADER_BYTES) {
        reject("invalidFrame", "recordLength", "a record shorter than the 16-byte header is not a frame");
    }
    const cursor = new Cursor(bytes);
    const magic = cursor.take(4);
    for (let i = 0; i < 4; i++) {
        if (magic[i] !== CONTROL_MAGIC_BYTES[i]) reject("invalidFrame", "magic", "the control magic is ASCII OBCP");
    }
    const major = cursor.u8();
    const minor = cursor.u8();
    const opcodeValue = cursor.u16();
    const flags = cursor.u16();
    const payloadLength = cursor.u16();
    const requestIdValue = cursor.u32();

    if (major !== WIRE_MAJOR) {
        reject("incompatibleVersion", "unsupportedMajor", `wire major ${major} is not this contract`);
    }
    if (minor !== WIRE_MINOR) {
        reject("incompatibleVersion", "unsupportedMinor", `wire minor ${minor} is above the device's`);
    }
    if (payloadLength > MAX_CONTROL_PAYLOAD) {
        reject("invalidFrame", "payloadLength", `a payload of ${payloadLength} bytes overflows the hard maximum frame`);
    }
    if (payloadLength !== bytes.length - CONTROL_HEADER_BYTES) {
        reject("invalidFrame", "payloadLength", "the payload length disagrees with the record length");
    }
    const limit = options.maximumFrameBytes;
    if (limit !== undefined && bytes.length > limit) {
        reject("invalidFrame", "frameBounds", `a ${bytes.length}-byte frame is outside the negotiated bounds`);
    }
    if ((flags & ~HEADER_FLAG_MASK) !== 0) {
        reject("invalidDescriptor", "unsupportedFlags", "header flags 3..15 are zero");
    }
    if (requestIdValue === 0) {
        reject("invalidDescriptor", "zeroRequestId", "a zero-RequestId frame is unanswerable and closes the stream");
    }

    const response = (flags & HEADER_FLAG.response) !== 0;
    const error = (flags & HEADER_FLAG.error) !== 0;
    const more = (flags & HEADER_FLAG.more) !== 0;
    if (!response && flags !== 0) reject("invalidDescriptor", "unsupportedFlags", "requests have no flags");
    if (error && !response) reject("invalidDescriptor", "unsupportedFlags", "the error bit accompanies the response bit");
    if (more && (error || !PAGEABLE.includes(opcodeValue))) {
        reject("invalidDescriptor", "invalidCombination", "`more` is valid only on a paged Capabilities, catalog, or draft response");
    }

    const opcodeName = OPCODE_NAME.get(opcodeValue);
    if (opcodeName === undefined) {
        reject("unsupportedCapability", "opcode", `opcode 0x${opcodeValue.toString(16)} is not registered`);
    }
    const payload = cursor.take(payloadLength);
    const body = error
        ? { kind: "Error" as const, error: readErrorBody(payload) }
        : readBody(opcodeName, response, more, payload, options);
    return {
        opcode: OPCODE[opcodeName],
        opcodeName,
        requestId: makeRequestId(requestIdValue),
        response,
        error,
        more,
        body,
    };
}

function empty(payload: Uint8Array, what: string): void {
    if (payload.length !== 0) reject("invalidFrame", "trailingBytes", `${what} has an empty payload`);
}

function readBody(
    opcode: OpcodeName,
    response: boolean,
    more: boolean,
    payload: Uint8Array,
    context: BodyContext,
): ControlBody {
    switch (opcode) {
        case "Hello":
            return response
                ? { kind: "Capabilities", capabilities: readCapabilities(payload) }
                : { kind: "Hello", hello: readHello(payload) };
        case "StartUpload":
            return response
                ? { kind: "UploadAccepted", response: readUploadAccepted(payload) }
                : { kind: "StartUpload", request: readStartUpload(payload) };
        case "CheckpointUpload":
            return response
                ? { kind: "CheckpointAccepted", response: readCheckpointResponse(payload) }
                : { kind: "CheckpointUpload", request: readCheckpointUpload(payload) };
        case "FinishUpload":
            return response
                ? { kind: "TerminalResult", result: readResultEnvelope(payload) }
                : { kind: "FinishUpload", request: readSessionOnly(payload, "FinishUpload") };
        case "StartDownload":
            return response
                ? { kind: "DownloadAccepted", response: readDownloadAccepted(payload) }
                : { kind: "StartDownload", request: readStartDownload(payload) };
        case "FinishDownload":
            if (response) {
                empty(payload, "the FinishDownload response");
                return { kind: "Empty" };
            }
            return { kind: "FinishDownload", request: readFinishDownload(payload) };
        case "AbortSession":
            return response
                ? { kind: "AbortSessionResult", response: readAbortSessionResponse(payload) }
                : { kind: "AbortSession", request: readAbortSession(payload) };
        case "BeginDraft":
            return response
                ? { kind: "BeginDraftAccepted", response: readBeginDraftResponse(payload) }
                : { kind: "BeginDraft", request: readBeginDraft(payload) };
        case "StartDraftPart":
            return response
                ? { kind: "DraftPartAccepted", response: readDraftPartAccepted(payload) }
                : { kind: "StartDraftPart", request: readStartDraftPart(payload) };
        case "FinalizeDraft":
            return response
                ? { kind: "FinalizeDraftAccepted", response: readFinalizeDraftResponse(payload) }
                : { kind: "FinalizeDraft", request: readFinalizeDraft(payload) };
        case "QueryOperation":
            return response
                ? { kind: "OperationStatus", response: readQueryOperationResponse(payload) }
                : { kind: "QueryOperation", request: readQueryOperation(payload) };
        case "QueryCatalog":
            return response
                ? { kind: "CatalogPage", response: readQueryCatalogResponse(payload, more) }
                : { kind: "QueryCatalog", request: readQueryCatalog(payload, context) };
        case "QueryDraft":
            return response
                ? { kind: "DraftPage", response: readQueryDraftResponse(payload, more, context) }
                : { kind: "QueryDraft", request: readQueryDraft(payload, context) };
        case "QueryWeatherRequest":
            if (response) return { kind: "WeatherRequestContext", response: readWeatherRequestContext(payload) };
            empty(payload, "the QueryWeatherRequest request");
            return { kind: "QueryWeatherRequest" };
        case "DeleteObject":
            return response
                ? { kind: "TerminalResult", result: readResultEnvelope(payload) }
                : { kind: "DeleteObject", request: readDeleteObject(payload) };
        case "SetMetadata":
            return response
                ? { kind: "TerminalResult", result: readResultEnvelope(payload) }
                : { kind: "SetMetadata", request: readSetMetadata(payload) };
        case "AbortOperation":
            return response
                ? { kind: "TerminalResult", result: readResultEnvelope(payload) }
                : { kind: "AbortOperation", request: readAbortOperation(payload) };
        case "InstallUpdate":
            return response
                ? { kind: "TerminalResult", result: readResultEnvelope(payload) }
                : { kind: "OperationOnObject", request: readOperationOnObject(payload, "InstallUpdate") };
        case "AcknowledgeRideImported":
            return response
                ? { kind: "TerminalResult", result: readResultEnvelope(payload) }
                : { kind: "OperationOnObject", request: readOperationOnObject(payload, "AcknowledgeRideImported") };
        case "GetDeviceStatus":
            if (response) return { kind: "DeviceStatus", response: readDeviceStatus(payload) };
            empty(payload, "the GetDeviceStatus request");
            return { kind: "GetDeviceStatus" };
        case "GetConfig":
            if (response) return { kind: "ConfigBlock", config: readConfigBlock(payload) };
            empty(payload, "the GetConfig request");
            return { kind: "GetConfig" };
        case "SetConfig":
            return { kind: "ConfigBlock", config: readConfigBlock(payload) };
        case "SetClock":
            return response
                ? { kind: "ClockStatus", response: readClockStatus(payload) }
                : { kind: "SetClock", request: readSetClock(payload) };
        case "ForgetBond":
            if (response) {
                empty(payload, "the ForgetBond response");
                return { kind: "Empty" };
            }
            return { kind: "ForgetBond", request: readForgetBond(payload) };
        case "Echo":
            return { kind: "Echo", payload: payload.slice() };
        case "ResetStore":
            return response
                ? { kind: "ResetStoreResult", response: readStoreIdMessage(payload, "the ResetStore response") }
                : { kind: "ResetStore", request: readStoreIdMessage(payload, "ResetStore") };
    }
}

export interface ControlEncodeOptions {
    /** The negotiated control frame maximum. A frame above it is unsendable on this link (§1). */
    readonly maximumFrameBytes?: number;
}

/**
 * Total encode. §1: "A request whose complete encoding exceeds the negotiated control frame is
 * unsendable. The client MUST NOT truncate, split, or drop a field to make it fit" — so an
 * over-long body is a refusal the caller has to handle, never a `u16` that quietly wraps.
 */
export const encodeControlFrame = (frame: ControlFrame, options: ControlEncodeOptions = {}): DosResult<Uint8Array> =>
    decoding(() => writeControlFrame(frame, options));

export function writeControlFrame(frame: ControlFrame, options: ControlEncodeOptions = {}): Uint8Array {
    const payload = writeBody(frame.body);
    if (payload.length > MAX_CONTROL_PAYLOAD) {
        reject(
            "invalidFrame",
            "payloadLength",
            `a ${payload.length}-byte payload exceeds the ${MAX_CONTROL_PAYLOAD}-byte maximum`,
        );
    }
    const limit = options.maximumFrameBytes;
    if (limit !== undefined && CONTROL_HEADER_BYTES + payload.length > limit) {
        reject("invalidFrame", "frameBounds", `the encoded frame exceeds the ${limit}-byte negotiated maximum`);
    }
    let flags = 0;
    if (frame.response) flags |= HEADER_FLAG.response;
    if (frame.error) flags |= HEADER_FLAG.error;
    if (frame.more) flags |= HEADER_FLAG.more;
    return new Writer(CONTROL_HEADER_BYTES + payload.length)
        .raw(CONTROL_MAGIC_BYTES)
        .u8(WIRE_MAJOR)
        .u8(WIRE_MINOR)
        .u16(frame.opcode)
        .u16(flags)
        .u16(payload.length)
        .u32(frame.requestId)
        .raw(payload)
        .finish();
}

function writeBody(body: ControlBody): Uint8Array {
    switch (body.kind) {
        case "Hello":
            return writeHello(body.hello);
        case "Capabilities":
            return writeCapabilities(body.capabilities);
        case "StartUpload":
            return writeStartUpload(body.request);
        case "UploadAccepted":
            return writeUploadAccepted(body.response);
        case "CheckpointUpload":
            return writeCheckpointUpload(body.request);
        case "CheckpointAccepted":
            return writeCheckpointResponse(body.response);
        case "FinishUpload":
            return writeSessionOnly(body.request);
        case "StartDownload":
            return writeStartDownload(body.request);
        case "DownloadAccepted":
            return writeDownloadAccepted(body.response);
        case "FinishDownload":
            return writeFinishDownload(body.request);
        case "AbortSession":
            return writeAbortSession(body.request);
        case "AbortSessionResult":
            return writeAbortSessionResponse(body.response);
        case "BeginDraft":
            return writeBeginDraft(body.request);
        case "BeginDraftAccepted":
            return writeBeginDraftResponse(body.response);
        case "StartDraftPart":
            return writeStartDraftPart(body.request);
        case "DraftPartAccepted":
            return writeDraftPartAccepted(body.response);
        case "FinalizeDraft":
            return writeFinalizeDraft(body.request);
        case "FinalizeDraftAccepted":
            return writeFinalizeDraftResponse(body.response);
        case "QueryOperation":
            return writeQueryOperation(body.request);
        case "OperationStatus":
            return writeQueryOperationResponse(body.response);
        case "QueryCatalog":
            return writeQueryCatalog(body.request);
        case "CatalogPage":
            return writeQueryCatalogResponse(body.response);
        case "QueryDraft":
            return writeQueryDraft(body.request);
        case "DraftPage":
            return writeQueryDraftResponse(body.response);
        case "DeleteObject":
            return writeDeleteObject(body.request);
        case "SetMetadata":
            return writeSetMetadata(body.request);
        case "AbortOperation":
            return writeAbortOperation(body.request);
        case "OperationOnObject":
            return writeOperationOnObject(body.request);
        case "TerminalResult":
            return writeResultEnvelope(body.result);
        case "WeatherRequestContext":
            return writeWeatherRequestContext(body.response);
        case "DeviceStatus":
            return writeDeviceStatus(body.response);
        case "ConfigBlock":
            return writeConfigBlock(body.config);
        case "SetClock":
            return writeSetClock(body.request);
        case "ClockStatus":
            return writeClockStatus(body.response);
        case "ForgetBond":
            return writeForgetBond(body.request);
        case "Echo":
            return body.payload.slice();
        case "ResetStore":
        case "ResetStoreResult":
            return writeStoreIdMessage(body.kind === "ResetStore" ? body.request : body.response);
        case "Error":
            return writeErrorBody(body.error);
        case "QueryWeatherRequest":
        case "GetDeviceStatus":
        case "GetConfig":
        case "Empty":
            return new Uint8Array(0);
    }
}
