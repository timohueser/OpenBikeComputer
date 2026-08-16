/**
 * Hello, Capabilities, ResourceLimits and subject entries — Device_Object_Protocol_v3.md §5 — plus
 * the frame-limit derivation of §14.0.
 *
 * Discovery is paged with *requests*: each page is its own Hello under its own RequestId, and the
 * capability revision is the snapshot token that binds the pages together. So the decoder's job is
 * per-page consistency (the page kind, the index against the total, the count against the payload
 * length) and nothing stateful.
 *
 * The one cross-block rule §5 spells out is byte 54: it repeats the ResourceLimits block's own byte
 * 0, and "a client that observes a mismatch MUST reject that page and abandon discovery rather than
 * decode either block, because the two disagree about how to read the second."
 */

import { Cursor, Writer } from "./bytes";
import { identityIsZero, storeId, type StoreId } from "./ids";
import {
    DRAFT_PART_KIND_NAME,
    OBJECT_KIND_NAME,
    PERMITTED_DRAFT_PART_OPS,
    PERMITTED_SUBJECT_OPS,
    SCHEMA_VERSION,
    SUBJECT_OP,
    SUBJECT_OP_MASK,
    SUBJECT_POLICY_MASK,
    type DraftPartKindName,
    type ObjectKindName,
} from "./registry";
import { reject } from "./result";

export const HELLO_BYTES = 12;
export const CAPABILITIES_PREFIX_BYTES = 56;
export const RESOURCE_LIMITS_BYTES = 56;
export const SUBJECT_ENTRY_BYTES = 20;
export const SUBJECTS_PER_PAGE = 2;
export const MAX_SUBJECTS = 16;

export const MIN_CONTROL_FRAME = 192;
export const MAX_CONTROL_FRAME = 512;
export const MIN_STREAM_FRAME = 64;
export const MAX_STREAM_FRAME = 4096;
const CONTROL_HEADER_BYTES = 16;
export const MAX_CONTROL_PAYLOAD = MAX_CONTROL_FRAME - CONTROL_HEADER_BYTES;
export const RETAINED_RESULTS = 64;
export const METADATA_ENVELOPE_LIMIT = 128;
export const CATALOG_METADATA_LIMIT = 96;

export const PAGE_KIND = { resourceLimits: 0, subjects: 1 } as const;
export const LINK_KIND = { ble: 1, usb: 2, test: 3 } as const;

/** §5 status flags. */
export const STATUS_FLAG = {
    storeAvailable: 1 << 0,
    authenticated: 1 << 1,
    heavyTransferBusy: 1 << 2,
    developerUnlocked: 1 << 3,
} as const;
const STATUS_FLAG_MASK = 0x0f;

/** §5 command flags, bit 0 (QueryOperation) through bit 16 (ResetStore). */
export const COMMAND_FLAG = {
    queryOperation: 0,
    queryCatalog: 1,
    queryDraft: 2,
    queryWeatherRequest: 3,
    beginDraft: 4,
    startDraftPart: 5,
    finalizeDraft: 6,
    abortOperation: 7,
    installUpdate: 8,
    acknowledgeRideImported: 9,
    getDeviceStatus: 10,
    getConfig: 11,
    setConfig: 12,
    setClock: 13,
    forgetBond: 14,
    echo: 15,
    resetStore: 16,
} as const;
const COMMAND_FLAG_MASK = 0x0001_ffff;

export interface HelloRequest {
    readonly minimumWireMajor: number;
    readonly maximumWireMajor: number;
    readonly clientMaximumControlFrame: number;
    readonly clientMaximumStreamFrame: number;
    readonly clientFeatureFlags: number;
    readonly pageKind: number;
    readonly pageIndex: number;
}

export function decodeHello(bytes: Uint8Array): HelloRequest {
    const cursor = new Cursor(bytes);
    const minimumWireMajor = cursor.u8();
    const maximumWireMajor = cursor.u8();
    const clientMaximumControlFrame = cursor.u16();
    const clientMaximumStreamFrame = cursor.u16();
    const clientFeatureFlags = cursor.u32();
    const pageKind = cursor.u8();
    const pageIndex = cursor.u8();
    cursor.end("Hello");

    if (minimumWireMajor > maximumWireMajor) {
        reject("invalidDescriptor", "invalidCombination", "the minimum wire major is above the maximum");
    }
    if (clientFeatureFlags !== 0) {
        reject("invalidDescriptor", "reservedBits", "client feature flags are zero in v3.0");
    }
    if (pageKind !== PAGE_KIND.resourceLimits && pageKind !== PAGE_KIND.subjects) {
        reject("invalidDescriptor", "unknownEnum", `page kind ${pageKind} is not registered`);
    }
    if (pageKind === PAGE_KIND.resourceLimits && pageIndex !== 0) {
        reject("invalidDescriptor", "invalidCombination", "the resource page has only index zero");
    }
    return {
        minimumWireMajor,
        maximumWireMajor,
        clientMaximumControlFrame,
        clientMaximumStreamFrame,
        clientFeatureFlags,
        pageKind,
        pageIndex,
    };
}

export function encodeHello(hello: HelloRequest): Uint8Array {
    return new Writer(HELLO_BYTES)
        .u8(hello.minimumWireMajor)
        .u8(hello.maximumWireMajor)
        .u16(hello.clientMaximumControlFrame)
        .u16(hello.clientMaximumStreamFrame)
        .u32(hello.clientFeatureFlags)
        .u8(hello.pageKind)
        .u8(hello.pageIndex)
        .finish();
}

export interface ResourceLimits {
    readonly codecVersion: number;
    readonly blockLength: number;
    readonly logicalCatalogHeads: number;
    readonly normalActiveClaims: number;
    readonly resumableWorkSlots: number;
    readonly activeDraftParents: number;
    readonly draftPartsPerParent: number;
    readonly manifestChildren: number;
    readonly mountedMapFiles: number;
    readonly readerLeases: number;
    readonly retainedGenerations: number;
    readonly retainedTerminalResults: number;
    readonly inactiveWorkHorizon: number;
    readonly maximumSingleGenerationLength: bigint;
    readonly availableReservationBytes: bigint;
    readonly routeCatalogHeads: number;
    readonly tripCatalogHeads: number;
    readonly rideCatalogHeads: number;
    readonly weatherCatalogHeads: number;
    readonly volumeManifestCatalogHeads: number;
    readonly updatePackageCatalogHeads: number;
    readonly heavyStreamSessions: number;
    readonly maintenanceClaims: number;
    readonly activeRideSlots: number;
}

export interface SubjectEntry {
    readonly namespace: number;
    readonly kindCode: number;
    readonly kind: ObjectKindName | DraftPartKindName;
    readonly operationFlags: number;
    readonly policyFlags: number;
    readonly putSchemaVersion: number;
    readonly patchSchemaVersion: number;
    readonly catalogSchemaVersion: number;
    readonly maximumLength: bigint;
}

export type CapabilityPage =
    | { readonly kind: "resourceLimits"; readonly limits: ResourceLimits }
    | { readonly kind: "subjects"; readonly entries: readonly SubjectEntry[] };

export interface Capabilities {
    readonly selectedWireMajor: number;
    readonly storageFormatVersion: number;
    readonly statusFlags: number;
    readonly storeId: StoreId;
    readonly negotiatedControlFrame: number;
    readonly negotiatedStreamFrame: number;
    readonly checkpointGranule: number;
    readonly retainedResultCapacity: number;
    readonly metadataEnvelopeLimit: number;
    readonly catalogMetadataLimit: number;
    readonly protocolMinimumControlFrame: number;
    readonly protocolMinimumStreamFrame: number;
    readonly linkKind: number;
    readonly authState: number;
    readonly capabilityRevision: number;
    readonly commandFlags: number;
    readonly totalSubjectCount: number;
    readonly returnedPageKind: number;
    readonly returnedPageIndex: number;
    readonly returnedSubjectCount: number;
    readonly totalPages: number;
    readonly resourceLimitsCodecVersion: number;
    readonly deviceWireMinor: number;
    readonly page: CapabilityPage;
}

export function decodeCapabilities(bytes: Uint8Array): Capabilities {
    const cursor = new Cursor(bytes);
    const selectedWireMajor = cursor.u8();
    const storageFormatVersion = cursor.u8();
    const statusFlags = cursor.u16();
    const store = storeId(cursor.take(16));
    const negotiatedControlFrame = cursor.u16();
    const negotiatedStreamFrame = cursor.u16();
    const checkpointGranule = cursor.u32();
    const retainedResultCapacity = cursor.u16();
    const metadataEnvelopeLimit = cursor.u16();
    const catalogMetadataLimit = cursor.u16();
    const protocolMinimumControlFrame = cursor.u16();
    const protocolMinimumStreamFrame = cursor.u16();
    const linkKind = cursor.u8();
    const authState = cursor.u8();
    const capabilityRevision = cursor.u32();
    const commandFlags = cursor.u32();
    const totalSubjectCount = cursor.u16();
    const returnedPageKind = cursor.u8();
    const returnedPageIndex = cursor.u8();
    const returnedSubjectCount = cursor.u8();
    const totalPages = cursor.u8();
    const resourceLimitsCodecVersion = cursor.u8();
    const deviceWireMinor = cursor.u8();

    if (selectedWireMajor !== 3) {
        reject("incompatibleVersion", "unsupportedMajor", `wire major ${selectedWireMajor} is not this contract`);
    }
    if ((statusFlags & ~STATUS_FLAG_MASK) !== 0) {
        reject("invalidDescriptor", "reservedBits", "status flags above bit 3 are zero");
    }
    if ((statusFlags & STATUS_FLAG.storeAvailable) === 0 && !identityIsZero(store)) {
        reject("invalidDescriptor", "reservedBits", "the StoreId is zero when store-available is clear");
    }
    if (retainedResultCapacity !== RETAINED_RESULTS) {
        reject("invalidDescriptor", "invalidCombination", `retained result capacity is exactly ${RETAINED_RESULTS}`);
    }
    if (metadataEnvelopeLimit !== METADATA_ENVELOPE_LIMIT || catalogMetadataLimit !== CATALOG_METADATA_LIMIT) {
        reject("invalidDescriptor", "invalidCombination", "the envelope limits are 128 and 96");
    }
    if (protocolMinimumControlFrame !== MIN_CONTROL_FRAME || protocolMinimumStreamFrame !== MIN_STREAM_FRAME) {
        reject("invalidDescriptor", "invalidCombination", "the protocol minima are 192 and 64");
    }
    if (linkKind !== LINK_KIND.ble && linkKind !== LINK_KIND.usb && linkKind !== LINK_KIND.test) {
        reject("invalidDescriptor", "unknownEnum", `link kind ${linkKind} is not registered`);
    }
    if (authState > 1) reject("invalidDescriptor", "unknownEnum", `auth state ${authState} is not registered`);
    if ((commandFlags & ~COMMAND_FLAG_MASK) !== 0) {
        reject("invalidDescriptor", "reservedBits", "command flags above bit 16 are zero");
    }
    if (totalSubjectCount > MAX_SUBJECTS) {
        reject("invalidDescriptor", "invalidCombination", `at most ${MAX_SUBJECTS} subjects may be advertised`);
    }
    if (returnedPageKind !== PAGE_KIND.resourceLimits && returnedPageKind !== PAGE_KIND.subjects) {
        reject("invalidDescriptor", "unknownEnum", `returned page kind ${returnedPageKind} is not registered`);
    }

    let page: CapabilityPage;
    if (returnedPageKind === PAGE_KIND.resourceLimits) {
        if (returnedPageIndex !== 0) {
            reject("invalidDescriptor", "invalidCombination", "the resource page has only index zero");
        }
        if (returnedSubjectCount !== 0) {
            reject("invalidDescriptor", "invalidCombination", "the returned subject count is zero on a resource page");
        }
        if (totalPages !== 1) {
            reject("invalidDescriptor", "invalidCombination", "the resource page kind has exactly one page");
        }
        const limits = decodeResourceLimits(cursor.take(RESOURCE_LIMITS_BYTES));
        if (limits.codecVersion !== resourceLimitsCodecVersion) {
            reject(
                "invalidDescriptor",
                "invalidCombination",
                "byte 54 and the ResourceLimits block's byte 0 disagree about how to read the block",
            );
        }
        page = { kind: "resourceLimits", limits };
    } else {
        const expectedPages = Math.ceil(totalSubjectCount / SUBJECTS_PER_PAGE);
        if (totalPages !== expectedPages) {
            reject("invalidDescriptor", "invalidCombination", `subject discovery has ${expectedPages} pages`);
        }
        if (returnedPageIndex >= Math.max(totalPages, 1)) {
            reject("invalidDescriptor", "invalidCombination", "the subject page index is beyond the last page");
        }
        if (returnedSubjectCount > SUBJECTS_PER_PAGE) {
            reject("invalidDescriptor", "invalidCombination", "a subject page returns at most two entries");
        }
        const entries: SubjectEntry[] = [];
        for (let i = 0; i < returnedSubjectCount; i++) entries.push(decodeSubjectEntry(cursor.take(SUBJECT_ENTRY_BYTES)));
        page = { kind: "subjects", entries };
    }
    cursor.end("Capabilities");

    return {
        selectedWireMajor,
        storageFormatVersion,
        statusFlags,
        storeId: store,
        negotiatedControlFrame,
        negotiatedStreamFrame,
        checkpointGranule,
        retainedResultCapacity,
        metadataEnvelopeLimit,
        catalogMetadataLimit,
        protocolMinimumControlFrame,
        protocolMinimumStreamFrame,
        linkKind,
        authState,
        capabilityRevision,
        commandFlags,
        totalSubjectCount,
        returnedPageKind,
        returnedPageIndex,
        returnedSubjectCount,
        totalPages,
        resourceLimitsCodecVersion,
        deviceWireMinor,
        page,
    };
}

export function encodeCapabilities(capabilities: Capabilities): Uint8Array {
    const writer = new Writer(CAPABILITIES_PREFIX_BYTES + RESOURCE_LIMITS_BYTES);
    writer
        .u8(capabilities.selectedWireMajor)
        .u8(capabilities.storageFormatVersion)
        .u16(capabilities.statusFlags)
        .raw(capabilities.storeId)
        .u16(capabilities.negotiatedControlFrame)
        .u16(capabilities.negotiatedStreamFrame)
        .u32(capabilities.checkpointGranule)
        .u16(capabilities.retainedResultCapacity)
        .u16(capabilities.metadataEnvelopeLimit)
        .u16(capabilities.catalogMetadataLimit)
        .u16(capabilities.protocolMinimumControlFrame)
        .u16(capabilities.protocolMinimumStreamFrame)
        .u8(capabilities.linkKind)
        .u8(capabilities.authState)
        .u32(capabilities.capabilityRevision)
        .u32(capabilities.commandFlags)
        .u16(capabilities.totalSubjectCount)
        .u8(capabilities.returnedPageKind)
        .u8(capabilities.returnedPageIndex)
        .u8(capabilities.returnedSubjectCount)
        .u8(capabilities.totalPages)
        .u8(capabilities.resourceLimitsCodecVersion)
        .u8(capabilities.deviceWireMinor);
    if (capabilities.page.kind === "resourceLimits") {
        writer.raw(encodeResourceLimits(capabilities.page.limits));
    } else {
        for (const entry of capabilities.page.entries) writer.raw(encodeSubjectEntry(entry));
    }
    return writer.finish();
}

export function decodeResourceLimits(bytes: Uint8Array): ResourceLimits {
    const cursor = new Cursor(bytes);
    const codecVersion = cursor.u8();
    const blockLength = cursor.u8();
    const flags = cursor.u16();
    if (blockLength !== RESOURCE_LIMITS_BYTES) {
        reject("invalidDescriptor", "invalidCombination", `the ResourceLimits block length is ${RESOURCE_LIMITS_BYTES}`);
    }
    if (flags !== 0) reject("invalidDescriptor", "reservedBits", "ResourceLimits flags are zero");
    const logicalCatalogHeads = cursor.u16();
    const normalActiveClaims = cursor.u8();
    const resumableWorkSlots = cursor.u8();
    const activeDraftParents = cursor.u8();
    const draftPartsPerParent = cursor.u8();
    const manifestChildren = cursor.u8();
    const mountedMapFiles = cursor.u8();
    const readerLeases = cursor.u8();
    const retainedGenerations = cursor.u8();
    const retainedTerminalResults = cursor.u16();
    const inactiveWorkHorizon = cursor.u16();
    // Byte 18 held journal capacity; §5.1 reserves it and encodes it zero.
    cursor.zeros(2, "ResourceLimits byte 18");
    const maximumSingleGenerationLength = cursor.u64();
    const availableReservationBytes = cursor.u64();
    const routeCatalogHeads = cursor.u16();
    const tripCatalogHeads = cursor.u16();
    const rideCatalogHeads = cursor.u16();
    const weatherCatalogHeads = cursor.u16();
    const volumeManifestCatalogHeads = cursor.u16();
    const updatePackageCatalogHeads = cursor.u16();
    const heavyStreamSessions = cursor.u8();
    const maintenanceClaims = cursor.u8();
    const activeRideSlots = cursor.u8();
    cursor.zeros(5, "ResourceLimits bytes 51..55");
    cursor.end("ResourceLimits");
    return {
        codecVersion,
        blockLength,
        logicalCatalogHeads,
        normalActiveClaims,
        resumableWorkSlots,
        activeDraftParents,
        draftPartsPerParent,
        manifestChildren,
        mountedMapFiles,
        readerLeases,
        retainedGenerations,
        retainedTerminalResults,
        inactiveWorkHorizon,
        maximumSingleGenerationLength,
        availableReservationBytes,
        routeCatalogHeads,
        tripCatalogHeads,
        rideCatalogHeads,
        weatherCatalogHeads,
        volumeManifestCatalogHeads,
        updatePackageCatalogHeads,
        heavyStreamSessions,
        maintenanceClaims,
        activeRideSlots,
    };
}

export function encodeResourceLimits(limits: ResourceLimits): Uint8Array {
    return new Writer(RESOURCE_LIMITS_BYTES)
        .u8(limits.codecVersion)
        .u8(limits.blockLength)
        .u16(0)
        .u16(limits.logicalCatalogHeads)
        .u8(limits.normalActiveClaims)
        .u8(limits.resumableWorkSlots)
        .u8(limits.activeDraftParents)
        .u8(limits.draftPartsPerParent)
        .u8(limits.manifestChildren)
        .u8(limits.mountedMapFiles)
        .u8(limits.readerLeases)
        .u8(limits.retainedGenerations)
        .u16(limits.retainedTerminalResults)
        .u16(limits.inactiveWorkHorizon)
        .zeros(2)
        .u64(limits.maximumSingleGenerationLength)
        .u64(limits.availableReservationBytes)
        .u16(limits.routeCatalogHeads)
        .u16(limits.tripCatalogHeads)
        .u16(limits.rideCatalogHeads)
        .u16(limits.weatherCatalogHeads)
        .u16(limits.volumeManifestCatalogHeads)
        .u16(limits.updatePackageCatalogHeads)
        .u8(limits.heavyStreamSessions)
        .u8(limits.maintenanceClaims)
        .u8(limits.activeRideSlots)
        .zeros(5)
        .finish();
}

export function decodeSubjectEntry(bytes: Uint8Array): SubjectEntry {
    const cursor = new Cursor(bytes);
    const namespace = cursor.u8();
    cursor.zeros(1, "subject entry byte 1");
    const kindCode = cursor.u16();
    const operationFlags = cursor.u16();
    const policyFlags = cursor.u16();
    const putSchemaVersion = cursor.u8();
    const patchSchemaVersion = cursor.u8();
    const catalogSchemaVersion = cursor.u8();
    cursor.zeros(1, "subject entry byte 11");
    const maximumLength = cursor.u64();
    cursor.end("subject entry");

    if ((operationFlags & ~SUBJECT_OP_MASK) !== 0) {
        reject("invalidDescriptor", "reservedBits", "subject operation flags above bit 6 are zero");
    }
    if ((policyFlags & ~SUBJECT_POLICY_MASK) !== 0) {
        reject("invalidDescriptor", "reservedBits", "subject policy flags above bit 3 are zero");
    }

    let kind: ObjectKindName | DraftPartKindName;
    if (namespace === 1) {
        const objectKind = OBJECT_KIND_NAME.get(kindCode);
        if (objectKind === undefined) {
            reject("invalidDescriptor", "unknownEnum", `ObjectKind ${kindCode} must not be advertised`);
        }
        kind = objectKind;
        if ((operationFlags & ~PERMITTED_SUBJECT_OPS[objectKind]) !== 0) {
            reject(
                "invalidDescriptor",
                "invalidCombination",
                `${objectKind} may not advertise operation flags 0x${operationFlags.toString(16)}`,
            );
        }
        const setsMetadata = (operationFlags & SUBJECT_OP.setMetadata) !== 0;
        const expectedPatch = setsMetadata ? SCHEMA_VERSION.patch : 0;
        if (patchSchemaVersion !== expectedPatch) {
            reject(
                "invalidDescriptor",
                "invalidCombination",
                `the patch schema version is ${expectedPatch} when set-metadata is ${setsMetadata ? "set" : "clear"}`,
            );
        }
    } else if (namespace === 2) {
        const partKind = DRAFT_PART_KIND_NAME.get(kindCode);
        if (partKind === undefined) {
            reject("invalidDescriptor", "unknownEnum", `DraftPartKind ${kindCode} is not registered`);
        }
        kind = partKind;
        if ((operationFlags & ~PERMITTED_DRAFT_PART_OPS) !== 0) {
            reject(
                "invalidDescriptor",
                "invalidCombination",
                "a draft-part subject advertises put and optionally resumable upload",
            );
        }
        if (putSchemaVersion !== 0 || patchSchemaVersion !== 0 || catalogSchemaVersion !== 0) {
            reject("invalidDescriptor", "invalidCombination", "draft-part subject schema versions are zero");
        }
    } else {
        return reject("invalidDescriptor", "unknownEnum", `subject namespace ${namespace} is not registered`);
    }

    return {
        namespace,
        kindCode,
        kind,
        operationFlags,
        policyFlags,
        putSchemaVersion,
        patchSchemaVersion,
        catalogSchemaVersion,
        maximumLength,
    };
}

export function encodeSubjectEntry(entry: SubjectEntry): Uint8Array {
    return new Writer(SUBJECT_ENTRY_BYTES)
        .u8(entry.namespace)
        .u8(0)
        .u16(entry.kindCode)
        .u16(entry.operationFlags)
        .u16(entry.policyFlags)
        .u8(entry.putSchemaVersion)
        .u8(entry.patchSchemaVersion)
        .u8(entry.catalogSchemaVersion)
        .u8(0)
        .u64(entry.maximumLength)
        .finish();
}

export type FrameLimitOutcome = "negotiated" | "belowProtocolMinimum" | "undeliverable";

export interface FrameLimitDerivation {
    readonly outcome: FrameLimitOutcome;
    readonly negotiated: number;
}

/**
 * §14.0: limits are derived from the link, then negotiated, and they fail closed.
 *
 * The control channel has two failure floors rather than one. Below 192 bytes no negotiation is
 * possible and Hello is answered `resourceLimit/minimumControlFrame`; below 64 — the header plus a
 * text-free ErrorBody — that refusal is itself undeliverable and the adapter disconnects instead of
 * truncating an error. The stream channel has only the 64-byte floor, because its refusal travels
 * on the control channel.
 */
export function negotiateFrameLimit(
    channel: "control" | "stream",
    transportCeiling: number,
    clientMaximum: number,
    deviceMaximum: number,
): FrameLimitDerivation {
    if (channel === "control") {
        if (transportCeiling < MIN_STREAM_FRAME) return { outcome: "undeliverable", negotiated: 0 };
        if (transportCeiling < MIN_CONTROL_FRAME) return { outcome: "belowProtocolMinimum", negotiated: 0 };
    } else if (transportCeiling < MIN_STREAM_FRAME) {
        return { outcome: "belowProtocolMinimum", negotiated: 0 };
    }
    return { outcome: "negotiated", negotiated: Math.min(transportCeiling, clientMaximum, deviceMaximum) };
}

/** §14.0: one ATT Write Request or indication value carries at most `ATT_MTU - 3` bytes. */
export const bleControlCeiling = (attMtu: number): number => attMtu - 3;
