/**
 * Superseded — see #1256. Do not extend.
 *
 * Device Object Protocol v3 — the browser/desktop client's wire codec.
 *
 * Written from the normative tables in `specs/Device_Object_Protocol_v3.md`,
 * `specs/Device_Object_Registries_v2.md` and `specs/Device_Object_System_v2.md`, and pinned against
 * the shared vectors in `specs/vectors/device-object-v2/` by `vectors.test.ts`. It is one of the
 * three independent implementations DOS1 requires; that independence is the point, so nothing here
 * is generated from — or derived by reading — the Rust or Swift codec.
 *
 * **Two naming rules, and they are load-bearing:**
 *
 * - `decode*` and `encode*` are **total**. They return a `DosResult` and never throw on any input,
 *   because a client that crashes on a bad frame cannot report the failure the frame was describing.
 *   Everything this barrel exports obeys that.
 * - `read*` and `write*` throw a `DecodeFault` and are package-internal. They are the straight-line
 *   field readers the total entry points wrap, and they are deliberately not re-exported here: one
 *   surface, one contract, no second door into the codec that behaves differently.
 *
 * The export list is explicit rather than a `export *` barrel, following the narrowing this repo
 * did to the builder's module exports in #1348.
 */

// --- identities (system contract) ------------------------------------------------------------
export {
    IDENTITY_BYTES,
    U64_V30_BOUND,
    deviceSerial,
    draftPartRef,
    draftRevision,
    identityEquals,
    identityIsZero,
    logicalObjectId,
    operationId,
    pageCursor,
    partKey,
    requestId,
    revision,
    sessionId,
    storeId,
    weatherRequestId,
    withinV30Bound,
    type DeviceSerial,
    type DraftPartRef,
    type DraftRevision,
    type LogicalObjectId,
    type OperationId,
    type PageCursor,
    type PartKey,
    type RequestId,
    type Revision,
    type SessionId,
    type StoreId,
    type WeatherRequestId,
} from "./ids";

// --- the failure taxonomy (§12) ----------------------------------------------------------------
export {
    CATEGORY,
    DETAILS,
    GUIDANCE,
    OWNER,
    detailName,
    unwrap,
    type CategoryName,
    type DosError,
    type DosResult,
} from "./result";

// --- bytes ---------------------------------------------------------------------------------
export { bytesToHex, hexToBytes } from "./bytes";

// --- CRC-32/IEEE (§1) -------------------------------------------------------------------------
export { CRC32_CHECK_INPUT, CRC32_CHECK_VALUE, crc32 } from "./crc32";

// --- the control frame (§2, §4) ----------------------------------------------------------------
export {
    CONTROL_HEADER_BYTES,
    HEADER_FLAG,
    OPCODE,
    OPCODE_NAME,
    WIRE_MAJOR,
    WIRE_MINOR,
    decodeControlFrame,
    encodeControlFrame,
    type ControlBody,
    type ControlDecodeOptions,
    type ControlEncodeOptions,
    type ControlFrame,
    type Opcode,
    type OpcodeName,
} from "./frame";

// --- the stream frame (§13) --------------------------------------------------------------------
export {
    FAULT_BODY_BYTES,
    FAULT_DISPOSITION,
    MAX_STREAM_FRAME_BYTES,
    STREAM_DIRECTION,
    STREAM_FLAG,
    STREAM_HEADER_BYTES,
    decodeStreamFrame,
    encodeStreamFrame,
    type StreamDecodeOptions,
    type StreamFault,
    type StreamFrame,
} from "./stream";

// --- discovery and frame limits (§5, §14.0) ----------------------------------------------------
export {
    CATALOG_METADATA_LIMIT,
    COMMAND_FLAG,
    LINK_KIND,
    MAX_CONTROL_FRAME,
    MAX_CONTROL_PAYLOAD,
    MAX_STREAM_FRAME,
    MAX_SUBJECTS,
    METADATA_ENVELOPE_LIMIT,
    MIN_CONTROL_FRAME,
    MIN_STREAM_FRAME,
    PAGE_KIND,
    RETAINED_RESULTS,
    STATUS_FLAG,
    SUBJECTS_PER_PAGE,
    bleControlCeiling,
    decodeCapabilities,
    decodeSubjectEntry,
    negotiateFrameLimit,
    type Capabilities,
    type CapabilityPage,
    type FrameLimitDerivation,
    type FrameLimitOutcome,
    type HelloRequest,
    type ResourceLimits,
    type SubjectEntry,
} from "./capabilities";

// --- the error body (§12) ----------------------------------------------------------------------
export {
    ERROR_BODY_PREFIX_BYTES,
    MAX_ERROR_TEXT_BYTES,
    PRESENCE,
    decodeErrorBody,
    errorText,
    isRetainedTerminalReplay,
    type ErrorBody,
} from "./errorBody";

// --- metadata envelopes (§2.2) -----------------------------------------------------------------
export {
    BASE_TAG_MASK,
    CRITICAL_BIT,
    decodeMetadataEnvelope,
    renderDiagnosticText,
    type EnvelopeContext,
    type MetadataEnvelope,
    type MetadataField,
    type MetadataValue,
} from "./metadata";

// --- the registries --------------------------------------------------------------------------
export {
    DRAFT_PART_KIND,
    ENVELOPE_CEILING,
    ENVELOPE_HEADER_BYTES,
    OBJECT_KIND,
    PERMITTED_DRAFT_PART_OPS,
    PERMITTED_SUBJECT_OPS,
    SCHEMA_VERSION,
    SEMANTIC_DETAILS,
    SUBJECT_OP,
    SUBJECT_POLICY,
    metadataSchema,
    semanticDetailName,
    type DraftPartKind,
    type DraftPartKindName,
    type MetadataBounds,
    type MetadataFieldSpec,
    type MetadataSchema,
    type ObjectKind,
    type ObjectKindName,
    type SchemaRole,
} from "./registry";

// --- message bodies: the shapes a `ControlBody` hands a caller, and the enums that read them ----
export {
    ABORT_REASON,
    ACCEPT_FLAG,
    CATALOG_ENTRY_PREFIX_BYTES,
    CATALOG_FLAG,
    CATALOG_PAGE_PREFIX_BYTES,
    CLOCK_SOURCE,
    CLOCK_STATE,
    CONFIG_BLOCK_BYTES,
    DEVICE_STATUS_FLAG,
    DOWNLOAD_FLAG,
    DRAFT_ENTRY_BYTES,
    DRAFT_FLAG,
    DRAFT_PAGE_FLAG,
    DRAFT_PART_STATE,
    FORGET_BOND_SCOPE,
    MAX_CATALOG_ENTRIES_PER_PAGE,
    MAX_DEVICE_NAME_BYTES,
    MAX_DRAFT_PAGE_ENTRIES,
    MAX_DRAFT_PARTS,
    MOUNT_CLASS,
    MUTATION_FLAG,
    OPERATION_STATE,
    OUTCOME,
    PHASE,
    PROGRESS_FLAG,
    RESULT_TYPE,
    RESUME,
    TARGET_MODE,
    UNIT_FLAG,
    WEATHER_CONTEXT_STATE,
    WEATHER_FLAG,
    decodeConfigBlock,
    validateResetStoreEcho,
    type AbortOperationRequest,
    type AbortResult,
    type AbortSessionRequest,
    type AbortSessionResponse,
    type BeginDraftAcceptance,
    type BeginDraftRequest,
    type BodyContext,
    type CatalogEntry,
    type CheckpointUploadRequest,
    type CheckpointUploadResponse,
    type ClockStatus,
    type ConfigBlock,
    type DeviceStatus,
    type DispositionResponse,
    type DownloadAccepted,
    type DraftEntry,
    type DraftPartAcceptance,
    type DraftPartResult,
    type FinalizeDraftAcceptance,
    type FinalizeDraftRequest,
    type FinishDownloadRequest,
    type ForgetBondRequest,
    type MutationTarget,
    type ObjectResult,
    type OperationOnObject,
    type OperationProgress,
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

// --- canonical intent (§11) --------------------------------------------------------------------
export {
    INTENT_CODEC_VERSION,
    INTENT_PREFIX_BYTES,
    INTENT_TAG,
    canonicalIntent,
    intentDigest,
    type IntentSource,
} from "./intent";
