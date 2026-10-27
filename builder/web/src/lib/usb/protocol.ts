/**
 * Protocol v4's shared vocabulary in TypeScript: the opcode, kind, flag and error tables, the
 * decoded response shapes, and the few byte helpers the client and its parsers share.
 *
 * `specs/FLAT_Store_Protocol.md` is the sole authority. The wire codec itself is the Rust
 * `obc_link::flat` client, which the browser runs through the wasm bridge.
 *
 * Every `u64` is a `bigint`, never a `number`: `Number` loses integers above 2^53, and a codec that
 * truncated an identity would be unrecoverably wrong. {@link toSafeNumber} is the one narrowing, for
 * a caller that genuinely needs a JS length.
 */

/** The control frame header. */
export const HEADER_LEN = 16;

/** The stream frame. A stream record is this followed by exactly `payload length` bytes. */
export const STREAM_HEADER_LEN = 16;

/** `StoreId` plus commit sequence, ahead of a `LIST` page's entries. */
export const LIST_PREFIX_LEN = 24;

/** One `LIST` entry. */
export const LIST_ENTRY_LEN = 88;

/** The opcode table. Nine, and there is no generic forwarding path. */
export const Opcode = {
    List: 0x01,
    Status: 0x02,
    Get: 0x03,
    Put: 0x04,
    Remove: 0x05,
    Cancel: 0x06,
    Arm: 0x07,
    Format: 0x08,
    ArchiveRide: 0x09,
} as const;
export type Opcode = (typeof Opcode)[keyof typeof Opcode];

const OPCODE_NAMES: Readonly<Record<Opcode, string>> = {
    [Opcode.List]: "LIST",
    [Opcode.Status]: "STATUS",
    [Opcode.Get]: "GET",
    [Opcode.Put]: "PUT",
    [Opcode.Remove]: "REMOVE",
    [Opcode.Cancel]: "CANCEL",
    [Opcode.Arm]: "ARM",
    [Opcode.Format]: "FORMAT",
    [Opcode.ArchiveRide]: "ARCHIVE_RIDE",
};

/** The spec's own name for an opcode, for a message a person will read. */
export function opcodeName(opcode: number): string {
    return OPCODE_NAMES[opcode as Opcode] ?? `opcode ${opcode}`;
}

/**
 * The object kinds, which the wire carries unchanged.
 *
 * `MapSetManifest` is retired: no producer writes it and the value is not reissued. It stays in the
 * table because the number is spent and a device that still holds one must be able to list and
 * remove it.
 */
export const ObjectKind = {
    Route: 1,
    Trip: 2,
    Ride: 3,
    /** OBCM. A map is one object carrying its terrain inside it. */
    MapShard: 5,
    /** Retired. Listable, removable, never written. */
    MapSetManifest: 6,
    UpdatePackage: 7,
    /** Extents owned by the store, payload written by the bootloader. */
    RollbackReserve: 8,
    Metadata: 9,
} as const;
export type ObjectKind = (typeof ObjectKind)[keyof typeof ObjectKind];

/** The entry flags, as a `LIST` entry reports them. No client sets one. */
export const EntryFlags = {
    /** A ride the device is recording. Its length and CRC are zero until the commit that ends it. */
    Recording: 1 << 0,
    /** A previous revision a retaining replace left behind. */
    Retained: 1 << 1,
    /** Extents held for the bootloader. The store did not write these bytes. */
    Reserved: 1 << 2,
    /** The exact immutable route payload has been accepted by Navigator. */
    AssistantAccepted: 1 << 3,
} as const;

/** `0` names no object, and is what a `PUT` sends to create one. */
export const NO_OBJECT = 0n;

/** `0` in a `GET`'s revision field takes the current head. */
export const HEAD_REVISION = 0n;

/** The error code table. Code `0` is invalid and is read as a malformed body. */
export const ErrorCode = {
    Unsupported: 1,
    InvalidFrame: 2,
    InvalidRequest: 3,
    NotFound: 4,
    RevisionConflict: 5,
    NoSpace: 6,
    ChecksumFailure: 7,
    MediaIo: 8,
    Busy: 9,
    Cancelled: 10,
    Rejected: 11,
    Internal: 12,
    CatalogChanged: 13,
    ReadOnly: 14,
} as const;
export type ErrorCode = (typeof ErrorCode)[keyof typeof ErrorCode];

/** The code names, exactly as the table spells them. */
const ERROR_CODE_NAMES: Readonly<Record<ErrorCode, string>> = {
    [ErrorCode.Unsupported]: "unsupported",
    [ErrorCode.InvalidFrame]: "invalidFrame",
    [ErrorCode.InvalidRequest]: "invalidRequest",
    [ErrorCode.NotFound]: "notFound",
    [ErrorCode.RevisionConflict]: "revisionConflict",
    [ErrorCode.NoSpace]: "noSpace",
    [ErrorCode.ChecksumFailure]: "checksumFailure",
    [ErrorCode.MediaIo]: "mediaIo",
    [ErrorCode.Busy]: "busy",
    [ErrorCode.Cancelled]: "cancelled",
    [ErrorCode.Rejected]: "rejected",
    [ErrorCode.Internal]: "internal",
    [ErrorCode.CatalogChanged]: "catalogChanged",
    [ErrorCode.ReadOnly]: "readOnly",
};

/**
 * Code-scoped details. `0` means "no narrower fact" everywhere.
 *
 * Grouped by code rather than flattened into one enum because the numbers *are* scoped: detail `3`
 * is `truncated` under `invalidFrame` and `badCombination` under `invalidRequest`.
 */
export const Detail = {
    unsupported: { opcode: 1, kind: 2, wireMajor: 3 },
    invalidFrame: { magic: 1, length: 2, truncated: 3, trailing: 4 },
    invalidRequest: { reservedBits: 1, unknownEnum: 2, badCombination: 3, streamOffset: 4 },
    notFound: { object: 1, revision: 2 },
    revisionConflict: { headDiffers: 1, headAbsent: 2 },
    noSpace: { extents: 1, catalogFull: 2, tooFragmented: 3 },
    checksumFailure: { payload: 1 },
    mediaIo: { read: 1, write: 2, sync: 3 },
    busy: { transfer: 1, holds: 2 },
    cancelled: { byClient: 1, byDevice: 2, linkLost: 3 },
    catalogChanged: { listing: 1 },
    readOnly: { catalogUnreadable: 1, revisionSpaceExhausted: 2, unformatted: 3 },
} as const;

/** The detail names of one code, for a message. `rejected`'s space belongs to a kind's validator. */
const DETAIL_NAMES: Partial<Record<ErrorCode, Readonly<Record<number, string>>>> = {
    [ErrorCode.Unsupported]: invert(Detail.unsupported),
    [ErrorCode.InvalidFrame]: invert(Detail.invalidFrame),
    [ErrorCode.InvalidRequest]: invert(Detail.invalidRequest),
    [ErrorCode.NotFound]: invert(Detail.notFound),
    [ErrorCode.RevisionConflict]: invert(Detail.revisionConflict),
    [ErrorCode.NoSpace]: invert(Detail.noSpace),
    [ErrorCode.ChecksumFailure]: invert(Detail.checksumFailure),
    [ErrorCode.MediaIo]: invert(Detail.mediaIo),
    [ErrorCode.Busy]: invert(Detail.busy),
    [ErrorCode.Cancelled]: invert(Detail.cancelled),
    [ErrorCode.CatalogChanged]: invert(Detail.catalogChanged),
    [ErrorCode.ReadOnly]: invert(Detail.readOnly),
};

function invert(table: Readonly<Record<string, number>>): Record<number, string> {
    const out: Record<number, string> = {};
    for (const [name, value] of Object.entries(table)) out[value] = name;
    return out;
}

/** One refusal, exactly as the 16-byte error body carries it. */
export interface Refusal {
    readonly code: ErrorCode;
    /** Code-scoped; `0` means no narrower fact. */
    readonly detail: number;
    /** Code-scoped; zero when the code defines none. */
    readonly context: bigint;
}

/** `unsupported`, `invalidFrame/magic` … — the spec's own words for one refusal. */
export function refusalName(r: Refusal): string {
    const detail = DETAIL_NAMES[r.code]?.[r.detail];
    return detail ? `${ERROR_CODE_NAMES[r.code]}/${detail}` : ERROR_CODE_NAMES[r.code];
}

/** `STATUS` and `GET` share a shape: an object and a revision. */
export interface ObjectRef {
    readonly objectId: bigint;
    readonly revision: bigint;
}

/** One catalog entry, 88 bytes. */
export interface CatalogEntry {
    readonly objectId: bigint;
    readonly revision: bigint;
    readonly payloadLength: bigint;
    readonly payloadCrc32: number;
    readonly kind: number;
    /** {@link EntryFlags}. A client reads these; it never sets them. */
    readonly flags: number;
    readonly displayName: string;
}

/** One `LIST` page: the identity prefix, the entries, and whether a further page exists. */
export interface ListPage {
    readonly storeId: string;
    readonly commitSequence: bigint;
    readonly entries: readonly CatalogEntry[];
    readonly more: boolean;
}

/** The three object states. */
export const ObjectState = { Absent: 0, Committed: 1, Superseded: 2 } as const;
export type ObjectState = (typeof ObjectState)[keyof typeof ObjectState];

/** The 24-byte STATUS response. Every head field is zero when `state` is `Absent`. */
export interface StatusResponse {
    readonly state: ObjectState;
    readonly headRevision: bigint;
    readonly headPayloadLength: bigint;
    readonly headPayloadCrc32: number;
}

/** The 24-byte GET response, sent once the last payload byte is on the transport. */
export interface GetResponse {
    readonly revisionServed: bigint;
    readonly payloadLength: bigint;
    readonly payloadCrc32: number;
}

/** The 32-byte PUT response. `objectId` is the assigned one when the request created an object. */
export interface PutResponse {
    readonly objectId: bigint;
    readonly revision: bigint;
    readonly payloadLength: bigint;
    readonly payloadCrc32: number;
}

export interface ArmResponse {
    readonly rollbackObjectId: bigint;
    readonly commitSequence: bigint;
}

/** The FORMAT response: the new identity, durable before the device reboots. */
export interface FormatResponse {
    readonly storeId: string;
}

/**
 * A `DataView` over exactly the bytes of `data` — `subarray` keeps the parent buffer, so the offset
 * and length have to be carried explicitly or every field read is off by the slice.
 *
 * Exported because `objects.ts` and `device/route.ts` parse the same little-endian layouts.
 */
export function viewOf(data: Uint8Array): DataView {
    return new DataView(data.buffer, data.byteOffset, data.byteLength);
}

/**
 * A `bigint` as a JS `number`, refusing anything `Number` would round. The narrowing exists for a
 * length that has to become a `Uint8Array` size or a progress figure; a payload above 2^53 bytes is
 * not a card that exists.
 */
export function toSafeNumber(value: bigint, what: string): number {
    if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
        throw new RangeError(`${what} is ${value}, beyond JavaScript's exact integer range.`);
    }
    return Number(value);
}

/** Lowercase hex, the form a `StoreId` is compared and logged in. */
export function hex(bytes: Uint8Array): string {
    let out = "";
    for (const byte of bytes) out += byte.toString(16).padStart(2, "0");
    return out;
}
