/**
 * Protocol v4's bytes in TypeScript: the control frame, the seven request and response bodies, the
 * stream frame and the error body.
 *
 * [`FLAT_Store_Protocol.md`](../../../../../specs/FLAT_Store_Protocol.md) §3 is the sole authority
 * and every offset below is transcribed from its tables. The Rust twin is
 * `firmware/obc-link/src/flat/wire.rs`, and both are pinned to the same frozen fixtures —
 * `specs/vectors/flat-store-v4/`, decoded and re-encoded byte for byte by `vectors.test.ts` here and
 * by `cargo test` there. Three languages, one set of bytes: a divergence here is a bug here, never a
 * reason to move a fixture.
 *
 * ## Two decisions worth stating
 *
 * **Every `u64` is a `bigint`, never a `number`.** `ObjectId`, `Revision`, payload lengths, commit
 * sequences and an error's context are 64 bits on the wire, and `Number` loses integers above 2^53.
 * A codec that silently truncated would be wrong in exactly the place — an identity — where being
 * wrong is unrecoverable. The alternative considered and rejected was `number` with a range check at
 * the boundary: it reads better at every call site and it makes the codec's correctness depend on
 * where it is called from. {@link toSafeNumber} is the one narrowing, used where a caller genuinely
 * needs a JS length (a byte offset into a `Uint8Array`) and can say so.
 *
 * **Decoding is total and typed.** A malformed record produces a {@link Refusal} carrying the
 * contract's own code and detail (§3.9), not a thrown string — the same shape the device would put
 * on the wire, which is what lets `vectors.test.ts` assert the *negative* fixtures against the same
 * table the firmware answers from. Only a programming error (an encode of an out-of-range field)
 * throws.
 *
 * This file holds no state and knows no policy. Whether a `PUT` may replace a ride, whether a
 * listing is stale, what a kind's validator thinks — all of that is the device's, and reading it
 * back is `client.ts`'s.
 */

/** The wire major this module implements. A transport fact (§4), never negotiated in a frame. */
export const WIRE_MAJOR = 4;

/** The four bytes every control frame opens with: ASCII `OBC4`. */
export const MAGIC = Object.freeze([0x4f, 0x42, 0x43, 0x34]);

/** §3.1's control frame header. */
export const HEADER_LEN = 16;

/** §3.8's stream frame. A stream record is this followed by exactly `payload length` bytes. */
export const STREAM_HEADER_LEN = 16;

/** §3.9's error response payload. Exactly this, never more and never less. */
export const ERROR_BODY_LEN = 16;

/** `StoreId` plus commit sequence, ahead of a `LIST` page's entries (§3.3). */
export const LIST_PREFIX_LEN = 24;

/** One `LIST` entry (§3.3). */
export const LIST_ENTRY_LEN = 88;

/** The display-name field's capacity, in UTF-8 bytes (§3.3, §3.6). */
export const NAME_CAPACITY = 48;

// --- opcodes, flags, kinds ------------------------------------------------------

/** §3.2's opcode table. Seven, and there is no generic forwarding path. */
export const Opcode = {
    List: 0x01,
    Status: 0x02,
    Get: 0x03,
    Put: 0x04,
    Remove: 0x05,
    Cancel: 0x06,
    Arm: 0x07,
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
};

/** The spec's own name for an opcode, for a message a person will read. */
export function opcodeName(opcode: number): string {
    return OPCODE_NAMES[opcode as Opcode] ?? `opcode ${opcode}`;
}

/** The exact payload length each opcode's **request** carries. Every message is a fixed layout. */
const REQUEST_BODY_LEN: Readonly<Record<Opcode, number>> = {
    [Opcode.List]: 32,
    [Opcode.Status]: 16,
    [Opcode.Get]: 16,
    [Opcode.Put]: 84,
    [Opcode.Remove]: 16,
    [Opcode.Cancel]: 4,
    [Opcode.Arm]: 16,
};

/** §3.1's flag bits. Requests carry none. */
export const Flags = {
    /** A successful response. */
    Response: 1 << 0,
    /** An error response; its payload is exactly one 16-byte error body. */
    Error: 1 << 1,
    /** A further `LIST` page exists. Valid on nothing else. */
    More: 1 << 2,
} as const;

/**
 * `FLAT_Store_Format.md` §3.1's object kinds, which the wire carries unchanged.
 *
 * `MapSetManifest` is retired with `OBCA_Spec.md` §5 (#1420) — no producer writes it and the value
 * is not reissued — and it stays in the table because the number is spent and a device that still
 * holds one must be able to list and remove it.
 */
export const ObjectKind = {
    Route: 1,
    Trip: 2,
    Ride: 3,
    WeatherBundle: 4,
    /** OBCM. Since v14 (#1420) a map is **one** object carrying its terrain inside it. */
    MapShard: 5,
    /** Retired (#1420). Listable, removable, never written. */
    MapSetManifest: 6,
    UpdatePackage: 7,
    /** Extents owned by the store, payload written by the bootloader (§4). */
    RollbackReserve: 8,
} as const;
export type ObjectKind = (typeof ObjectKind)[keyof typeof ObjectKind];

const OBJECT_KINDS: ReadonlySet<number> = new Set(Object.values(ObjectKind));

const KIND_NAMES: Readonly<Record<ObjectKind, string>> = {
    [ObjectKind.Route]: "route",
    [ObjectKind.Trip]: "trip",
    [ObjectKind.Ride]: "ride",
    [ObjectKind.WeatherBundle]: "weather bundle",
    [ObjectKind.MapShard]: "map",
    [ObjectKind.MapSetManifest]: "map set manifest",
    [ObjectKind.UpdatePackage]: "update package",
    [ObjectKind.RollbackReserve]: "rollback reserve",
};

/** A kind's word, for a sentence a rider reads. */
export function kindName(kind: number): string {
    return KIND_NAMES[kind as ObjectKind] ?? `kind ${kind}`;
}

/** `FLAT_Store_Format.md` §3's entry flags, as a `LIST` entry reports them. No client sets one. */
export const EntryFlags = {
    /** A ride the device is recording. Its length and CRC are zero until the commit that ends it. */
    Recording: 1 << 0,
    /** A previous revision a retaining replace left behind (§3.6). */
    Retained: 1 << 1,
    /** Extents held for the bootloader. The store did not write these bytes. */
    Reserved: 1 << 2,
} as const;

/** `0` names no object (`FLAT_Store_Format.md` §3), and is what a `PUT` sends to create one. */
export const NO_OBJECT = 0n;

/** `0` in a `GET`'s revision field takes the current head (§3.5). */
export const HEAD_REVISION = 0n;

// --- errors (§3.9) --------------------------------------------------------------

/** §3.9's code table. Code `0` is invalid and is read as a malformed body. */
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

const ERROR_CODES: ReadonlySet<number> = new Set(Object.values(ErrorCode));

/** §3.9's names, exactly as the table spells them. */
export const ERROR_CODE_NAMES: Readonly<Record<ErrorCode, string>> = {
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
 * §3.9's code-scoped details. `0` means "no narrower fact" everywhere.
 *
 * Grouped by code rather than flattened into one enum because the numbers *are* scoped: detail `3`
 * is `truncated` under `invalidFrame` and `badCombination` under `invalidRequest`, and a single
 * table would make those look like one value with two meanings.
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

/** One refusal, exactly as §3.9's 16-byte body carries it. */
export interface Refusal {
    readonly code: ErrorCode;
    /** Code-scoped; `0` means no narrower fact. */
    readonly detail: number;
    /** Code-scoped; zero when the code defines none. */
    readonly context: bigint;
}

/** A refusal with a detail and no context — the shape most of §3.9's table takes. */
export function refusal(code: ErrorCode, detail = 0, context = 0n): Refusal {
    return { code, detail, context };
}

/** `unsupported`, `invalidFrame/magic` … — the spec's own words for one refusal. */
export function refusalName(r: Refusal): string {
    const detail = DETAIL_NAMES[r.code]?.[r.detail];
    return detail ? `${ERROR_CODE_NAMES[r.code]}/${detail}` : ERROR_CODE_NAMES[r.code];
}

/** §3.9's 16 bytes. */
export function encodeErrorBody(r: Refusal): Uint8Array {
    const out = new Uint8Array(ERROR_BODY_LEN);
    const view = new DataView(out.buffer);
    view.setUint16(0, r.code, true);
    view.setUint16(2, r.detail, true);
    view.setBigUint64(4, r.context, true);
    return out;
}

/**
 * Decode §3.9's body. `null` for a body that is not one: the wrong length, a nonzero tail, or a
 * code this build has no entry for.
 *
 * An unknown code is deliberately **not** decoded into "some error": §3.9 says a receiver reads a
 * code it does not know as a failure it cannot classify, and `client.ts` reports exactly that. What
 * it must never do is treat it as success, which a lenient decode invites.
 */
export function decodeErrorBody(body: Uint8Array): Refusal | null {
    if (body.length !== ERROR_BODY_LEN || !isZero(body.subarray(12, 16))) return null;
    const view = viewOf(body);
    const code = view.getUint16(0, true);
    if (!ERROR_CODES.has(code)) return null;
    return { code: code as ErrorCode, detail: view.getUint16(2, true), context: view.getBigUint64(4, true) };
}

// --- the control frame (§3.1) ---------------------------------------------------

/** A decoded control frame's header. */
export interface Header {
    readonly opcode: number;
    readonly flags: number;
    readonly requestId: number;
    /** Where the payload starts and how long it is, as the header declared it. */
    readonly payloadLength: number;
}

/**
 * Why a control record produced no message.
 *
 * - `unanswerable` — there is no `RequestId` to echo: the record is shorter than a header, or its
 *   `RequestId` is zero (§3.1). A receiver emits nothing and closes that record stream.
 * - `refused` — the frame is answerable and wrong, and this refusal is the body of the error
 *   response it earns.
 */
export type ControlFailure =
    | { readonly kind: "unanswerable"; readonly why: string }
    | { readonly kind: "refused"; readonly requestId: number; readonly refusal: Refusal };

const unanswerable = (why: string): ControlFailure => ({ kind: "unanswerable", why });
const refused = (requestId: number, r: Refusal): ControlFailure => ({ kind: "refused", requestId, refusal: r });
const reservedBits = () => refusal(ErrorCode.InvalidRequest, Detail.invalidRequest.reservedBits);
const badCombination = () => refusal(ErrorCode.InvalidRequest, Detail.invalidRequest.badCombination);

/** Write §3.1's header into `out`. Throws only on a caller's own arithmetic error. */
export function encodeHeader(
    out: Uint8Array,
    opcode: number,
    flags: number,
    payloadLength: number,
    requestId: number,
): void {
    if (out.length < HEADER_LEN + payloadLength) {
        throw new RangeError(`a ${HEADER_LEN + payloadLength}-byte frame does not fit ${out.length} bytes.`);
    }
    if (payloadLength > 0xffff) throw new RangeError(`a control payload of ${payloadLength} bytes is above u16.`);
    out.set(MAGIC, 0);
    out[4] = WIRE_MAJOR;
    out[5] = opcode;
    const view = new DataView(out.buffer, out.byteOffset, out.byteLength);
    view.setUint16(6, flags, true);
    view.setUint16(8, payloadLength, true);
    view.setUint16(10, 0, true);
    view.setUint32(12, requestId >>> 0, true);
}

/** One whole control frame: header plus `body`. */
export function encodeControl(opcode: number, flags: number, requestId: number, body: Uint8Array): Uint8Array {
    const out = new Uint8Array(HEADER_LEN + body.length);
    encodeHeader(out, opcode, flags, body.length, requestId);
    out.set(body, HEADER_LEN);
    return out;
}

/** An error response: §3.1's header with `response|error`, and exactly one §3.9 body. */
export function encodeErrorResponse(opcode: number, requestId: number, r: Refusal): Uint8Array {
    return encodeControl(opcode, Flags.Response | Flags.Error, requestId, encodeErrorBody(r));
}

/**
 * Decode a control record's header, checking every framing rule of §3.1 that does not need to know
 * which direction the frame travelled.
 *
 * The `RequestId` is read **before** anything else can fail, because §3.1's whole distinction
 * between "close the stream" and "answer with an error" turns on whether there is an identifier to
 * echo.
 */
export function decodeHeader(record: Uint8Array): Header | ControlFailure {
    if (record.length < HEADER_LEN) return unanswerable(`a ${record.length}-byte record is shorter than a header.`);
    const view = viewOf(record);
    const requestId = view.getUint32(12, true);
    if (requestId === 0) return unanswerable("a zero RequestId is unanswerable.");
    for (let i = 0; i < 4; i++) {
        if (record[i] !== MAGIC[i]) {
            return refused(requestId, refusal(ErrorCode.InvalidFrame, Detail.invalidFrame.magic));
        }
    }
    if (record[4] !== WIRE_MAJOR) {
        return refused(requestId, refusal(ErrorCode.Unsupported, Detail.unsupported.wireMajor));
    }
    if (view.getUint16(10, true) !== 0) return refused(requestId, reservedBits());
    return {
        opcode: record[5],
        flags: view.getUint16(6, true),
        requestId,
        payloadLength: view.getUint16(8, true),
    };
}

/** True where `value` is a {@link ControlFailure} rather than the thing that was asked for. */
export function isFailure<T extends object>(value: T | ControlFailure): value is ControlFailure {
    return "kind" in value && (value.kind === "unanswerable" || value.kind === "refused");
}

// --- requests (the device's decode side, and the client's encode side) -----------

/** §3.3's cursor: the **pair**, plus the commit sequence the page was told. */
export interface ListCursor {
    readonly objectId: bigint;
    readonly revision: bigint;
    readonly commitSequence: bigint;
}

/** §3.3. `kind` of `null` lists every kind; `cursor` of `null` is a first page. */
export interface ListRequest {
    readonly kind: ObjectKind | null;
    readonly cursor: ListCursor | null;
}

/** §3.4 and §3.5 share a shape: an object and a revision. */
export interface ObjectRef {
    readonly objectId: bigint;
    readonly revision: bigint;
}

/** §3.6. */
export interface PutRequest {
    /** {@link NO_OBJECT} creates; anything else replaces. */
    readonly objectId: bigint;
    /** Zero when creating; the revision the device last reported when replacing. */
    readonly expectedRevision: bigint;
    readonly payloadLength: bigint;
    readonly payloadCrc32: number;
    readonly kind: ObjectKind;
    /** Ask the same commit to leave the displaced revision `RETAINED` (§3.6). */
    readonly retainPrevious: boolean;
    readonly displayName: string;
}

/** §3.8's `CANCEL`: the identifier of the transfer to drop. */
export interface CancelRequest {
    readonly transferRequestId: number;
}

/** §4's `ARM`: the update package to make the next boot. */
export interface ArmRequest {
    readonly packageObjectId: bigint;
    readonly expectedRevision: bigint;
}

/** One decoded request, tagged by its opcode. */
export type Request =
    | { readonly opcode: typeof Opcode.List; readonly body: ListRequest }
    | { readonly opcode: typeof Opcode.Status; readonly body: ObjectRef }
    | { readonly opcode: typeof Opcode.Get; readonly body: ObjectRef }
    | { readonly opcode: typeof Opcode.Put; readonly body: PutRequest }
    | { readonly opcode: typeof Opcode.Remove; readonly body: ObjectRef }
    | { readonly opcode: typeof Opcode.Cancel; readonly body: CancelRequest }
    | { readonly opcode: typeof Opcode.Arm; readonly body: ArmRequest };

/** A decoded request and the `RequestId` its answer must echo. */
export interface DecodedRequest {
    readonly requestId: number;
    readonly request: Request;
}

/**
 * Decode one whole control record as a **request** — what the device does with a host's bytes, and
 * what `loopback.ts` therefore needs.
 *
 * Total: every input is either a request or a {@link ControlFailure} the caller answers with.
 */
export function decodeRequest(record: Uint8Array): DecodedRequest | ControlFailure {
    const header = decodeHeader(record);
    if (isFailure(header)) return header;
    const { requestId } = header;
    if (!(header.opcode in REQUEST_BODY_LEN)) {
        return refused(requestId, refusal(ErrorCode.Unsupported, Detail.unsupported.opcode));
    }
    const opcode = header.opcode as Opcode;
    // "Requests carry no flags" (§3.1) — including the response bit, which is what makes a frame
    // looped back to its sender a refusal rather than something it might try to serve.
    if (header.flags !== 0) return refused(requestId, reservedBits());
    if (header.payloadLength !== REQUEST_BODY_LEN[opcode]) {
        return refused(requestId, refusal(ErrorCode.InvalidFrame, Detail.invalidFrame.length));
    }
    const carried = record.length - HEADER_LEN;
    if (carried < header.payloadLength) {
        return refused(requestId, refusal(ErrorCode.InvalidFrame, Detail.invalidFrame.truncated));
    }
    if (carried > header.payloadLength) {
        return refused(requestId, refusal(ErrorCode.InvalidFrame, Detail.invalidFrame.trailing));
    }

    const body = record.subarray(HEADER_LEN);
    const decoded = decodeRequestBody(opcode, body);
    if ("code" in decoded) return refused(requestId, decoded);
    return { requestId, request: decoded };
}

function decodeRequestBody(opcode: Opcode, body: Uint8Array): Request | Refusal {
    const view = viewOf(body);
    switch (opcode) {
        case Opcode.List: {
            const filter = view.getUint16(0, true);
            if (filter !== 0 && !OBJECT_KINDS.has(filter)) {
                // A filter naming a kind this major does not register is `unsupported`, exactly as
                // an unknown opcode is: the client asked for something with no table behind it.
                return refusal(ErrorCode.Unsupported, Detail.unsupported.kind);
            }
            const flags = view.getUint16(2, true);
            if ((flags & ~1) !== 0 || !isZero(body.subarray(4, 8))) return reservedBits();
            const cursor: ListCursor = {
                objectId: view.getBigUint64(8, true),
                revision: view.getBigUint64(16, true),
                commitSequence: view.getBigUint64(24, true),
            };
            const kind = filter === 0 ? null : (filter as ObjectKind);
            if ((flags & 1) === 0) {
                // "zero unless the cursor bit is set" — three fields, one rule.
                if (cursor.objectId !== 0n || cursor.revision !== 0n || cursor.commitSequence !== 0n) {
                    return badCombination();
                }
                return { opcode, body: { kind, cursor: null } };
            }
            return { opcode, body: { kind, cursor } };
        }
        case Opcode.Status: {
            const objectId = view.getBigUint64(0, true);
            // §3.4: a `STATUS` naming ObjectId zero is `invalidRequest` — the identity of the store
            // comes from `LIST`.
            if (objectId === NO_OBJECT) return badCombination();
            return { opcode, body: { objectId, revision: view.getBigUint64(8, true) } };
        }
        case Opcode.Get:
            return { opcode, body: { objectId: view.getBigUint64(0, true), revision: view.getBigUint64(8, true) } };
        case Opcode.Put: {
            const objectId = view.getBigUint64(0, true);
            const expectedRevision = view.getBigUint64(8, true);
            // §3.6: "Zero is not a wildcard in either field."
            if ((objectId !== NO_OBJECT) !== (expectedRevision !== 0n)) return badCombination();
            const kind = view.getUint16(28, true);
            if (!OBJECT_KINDS.has(kind)) return refusal(ErrorCode.Unsupported, Detail.unsupported.kind);
            const flags = view.getUint16(30, true);
            if ((flags & ~1) !== 0 || !isZero(body.subarray(33, 36))) return reservedBits();
            const name = decodeName(body[32], body.subarray(36, 36 + NAME_CAPACITY));
            if (typeof name !== "string") return name;
            return {
                opcode,
                body: {
                    objectId,
                    expectedRevision,
                    payloadLength: view.getBigUint64(16, true),
                    payloadCrc32: view.getUint32(24, true),
                    kind: kind as ObjectKind,
                    retainPrevious: (flags & 1) !== 0,
                    displayName: name,
                },
            };
        }
        case Opcode.Remove:
            return { opcode, body: { objectId: view.getBigUint64(0, true), revision: view.getBigUint64(8, true) } };
        case Opcode.Cancel:
            return { opcode, body: { transferRequestId: view.getUint32(0, true) } };
        case Opcode.Arm:
            return {
                opcode,
                body: { packageObjectId: view.getBigUint64(0, true), expectedRevision: view.getBigUint64(8, true) },
            };
    }
}

/**
 * §3.3 and §3.6 carry the same 49-byte name field: a length byte, then 48 bytes whose unused tail is
 * zero. The one rule beyond the spec's table is that the field is the UTF-8 it says it is — a menu
 * has nothing to do with bytes that are not, and a lossy decode would put replacement characters in
 * a rider's route list.
 */
function decodeName(length: number, field: Uint8Array): string | Refusal {
    if (length > NAME_CAPACITY) return badCombination();
    if (!isZero(field.subarray(length))) return reservedBits();
    try {
        return new TextDecoder("utf-8", { fatal: true }).decode(field.subarray(0, length));
    } catch {
        return badCombination();
    }
}

/** Write a `name_len u8` + zero-padded 48-byte field. Throws on a name the field cannot hold. */
function encodeName(out: Uint8Array, lengthAt: number, fieldAt: number, name: string): void {
    const bytes = new TextEncoder().encode(name);
    if (bytes.length > NAME_CAPACITY) {
        throw new RangeError(`a display name is at most ${NAME_CAPACITY} UTF-8 bytes, this one is ${bytes.length}.`);
    }
    out[lengthAt] = bytes.length;
    out.set(bytes, fieldAt);
}

// --- request encoders (the client's side) ----------------------------------------

/** §3.3's 32-byte request. */
export function encodeListRequest(requestId: number, request: ListRequest): Uint8Array {
    const body = new Uint8Array(REQUEST_BODY_LEN[Opcode.List]);
    const view = new DataView(body.buffer);
    view.setUint16(0, request.kind ?? 0, true);
    if (request.cursor) {
        view.setUint16(2, 1, true);
        view.setBigUint64(8, request.cursor.objectId, true);
        view.setBigUint64(16, request.cursor.revision, true);
        view.setBigUint64(24, request.cursor.commitSequence, true);
    }
    return encodeControl(Opcode.List, 0, requestId, body);
}

/** §3.4's 16-byte request. */
export function encodeStatusRequest(requestId: number, ref: ObjectRef): Uint8Array {
    return encodeControl(Opcode.Status, 0, requestId, encodeObjectRef(ref));
}

/** §3.5's 16-byte request. A revision of {@link HEAD_REVISION} takes the current head. */
export function encodeGetRequest(requestId: number, ref: ObjectRef): Uint8Array {
    return encodeControl(Opcode.Get, 0, requestId, encodeObjectRef(ref));
}

/** §3.7's 16-byte request. */
export function encodeRemoveRequest(requestId: number, ref: ObjectRef): Uint8Array {
    return encodeControl(Opcode.Remove, 0, requestId, encodeObjectRef(ref));
}

function encodeObjectRef(ref: ObjectRef): Uint8Array {
    const body = new Uint8Array(16);
    const view = new DataView(body.buffer);
    view.setBigUint64(0, ref.objectId, true);
    view.setBigUint64(8, ref.revision, true);
    return body;
}

/** §3.6's 84-byte request. */
export function encodePutRequest(requestId: number, request: PutRequest): Uint8Array {
    const body = new Uint8Array(REQUEST_BODY_LEN[Opcode.Put]);
    const view = new DataView(body.buffer);
    view.setBigUint64(0, request.objectId, true);
    view.setBigUint64(8, request.expectedRevision, true);
    view.setBigUint64(16, request.payloadLength, true);
    view.setUint32(24, request.payloadCrc32, true);
    view.setUint16(28, request.kind, true);
    view.setUint16(30, request.retainPrevious ? 1 : 0, true);
    encodeName(body, 32, 36, request.displayName);
    return encodeControl(Opcode.Put, 0, requestId, body);
}

/** §3.8's 4-byte request. */
export function encodeCancelRequest(requestId: number, request: CancelRequest): Uint8Array {
    const body = new Uint8Array(REQUEST_BODY_LEN[Opcode.Cancel]);
    new DataView(body.buffer).setUint32(0, request.transferRequestId >>> 0, true);
    return encodeControl(Opcode.Cancel, 0, requestId, body);
}

/** §4's 16-byte request. */
export function encodeArmRequest(requestId: number, request: ArmRequest): Uint8Array {
    const body = new Uint8Array(REQUEST_BODY_LEN[Opcode.Arm]);
    const view = new DataView(body.buffer);
    view.setBigUint64(0, request.packageObjectId, true);
    view.setBigUint64(8, request.expectedRevision, true);
    return encodeControl(Opcode.Arm, 0, requestId, body);
}

// --- responses -------------------------------------------------------------------

/** One catalog entry, §3.3's 88 bytes. */
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

/** §3.4's three states. */
export const ObjectState = { Absent: 0, Committed: 1, Superseded: 2 } as const;
export type ObjectState = (typeof ObjectState)[keyof typeof ObjectState];

/** §3.4's 24-byte response. Every head field is zero when `state` is `Absent`. */
export interface StatusResponse {
    readonly state: ObjectState;
    readonly headRevision: bigint;
    readonly headPayloadLength: bigint;
    readonly headPayloadCrc32: number;
}

/** §3.5's 24-byte response, sent once the last payload byte is on the transport. */
export interface GetResponse {
    readonly revisionServed: bigint;
    readonly payloadLength: bigint;
    readonly payloadCrc32: number;
}

/** §3.6's 32-byte response. `objectId` is the assigned one when the request created an object. */
export interface PutResponse {
    readonly objectId: bigint;
    readonly revision: bigint;
    readonly payloadLength: bigint;
    readonly payloadCrc32: number;
}

/** §4's 16-byte response. */
export interface ArmResponse {
    readonly rollbackObjectId: bigint;
    readonly commitSequence: bigint;
}

/** Every response body this protocol has, tagged by the opcode that produced it. */
export type Response =
    | { readonly opcode: typeof Opcode.List; readonly body: ListPage }
    | { readonly opcode: typeof Opcode.Status; readonly body: StatusResponse }
    | { readonly opcode: typeof Opcode.Get; readonly body: GetResponse }
    | { readonly opcode: typeof Opcode.Put; readonly body: PutResponse }
    | { readonly opcode: typeof Opcode.Remove; readonly body: { readonly commitSequence: bigint } }
    | { readonly opcode: typeof Opcode.Cancel; readonly body: { readonly cancelled: boolean } }
    | { readonly opcode: typeof Opcode.Arm; readonly body: ArmResponse };

/** A decoded response: the `RequestId` it echoes, and either a body or the refusal it carried. */
export type DecodedResponse =
    | { readonly requestId: number; readonly ok: true; readonly response: Response }
    | { readonly requestId: number; readonly ok: false; readonly refusal: Refusal; readonly opcode: number };

/**
 * Why a device→host record could not be read as a response.
 *
 * This is not §3.9's table: those are refusals the device *sent*, which decode fine. This is the
 * frame itself being unreadable, which under §3.1 means the two ends disagree about the wire and
 * the channel cannot be trusted for the next answer either.
 */
export class ResponseError extends Error {
    constructor(message: string) {
        super(message);
        this.name = "ResponseError";
    }
}

/**
 * Decode one control record as a **response** — what the client does with the device's bytes.
 *
 * Every failure throws {@link ResponseError} rather than returning a typed refusal, and the
 * asymmetry with {@link decodeRequest} is deliberate: a device has somewhere to put a refusal (an
 * error response under the offending `RequestId`), and a host has nowhere at all. §3.1 gives the
 * host no way to complain about a malformed answer, so the only honest move is to fail the waiter.
 */
export function decodeResponse(record: Uint8Array): DecodedResponse {
    const header = decodeHeader(record);
    if (isFailure(header)) {
        throw new ResponseError(
            header.kind === "unanswerable"
                ? `the device sent an unreadable control record: ${header.why}`
                : `the device sent a control record this client reads as ${refusalName(header.refusal)}.`,
        );
    }
    if ((header.flags & Flags.Response) === 0) {
        throw new ResponseError("the device sent a control frame with no response bit; there are no unsolicited ones.");
    }
    const body = record.subarray(HEADER_LEN);
    if (body.length !== header.payloadLength) {
        throw new ResponseError(
            `a response declared ${header.payloadLength} payload bytes and its record carries ${body.length}.`,
        );
    }
    if ((header.flags & Flags.Error) !== 0) {
        const r = decodeErrorBody(body);
        if (!r) throw new ResponseError("the device sent an error response whose body is not a §3.9 body.");
        return { requestId: header.requestId, ok: false, refusal: r, opcode: header.opcode };
    }
    if ((header.flags & ~(Flags.Response | Flags.More)) !== 0) {
        throw new ResponseError(`a response carries flags 0x${header.flags.toString(16)}; §3.1 defines three bits.`);
    }
    const more = (header.flags & Flags.More) !== 0;
    if (more && header.opcode !== Opcode.List) {
        throw new ResponseError(`the more bit is valid only on a LIST response, not on ${opcodeName(header.opcode)}.`);
    }
    return { requestId: header.requestId, ok: true, response: decodeResponseBody(header.opcode, body, more) };
}

function decodeResponseBody(opcode: number, body: Uint8Array, more: boolean): Response {
    const view = viewOf(body);
    const expect = (bytes: number) => {
        if (body.length !== bytes) {
            throw new ResponseError(
                `a ${opcodeName(opcode)} response is ${bytes} bytes, this one carries ${body.length}.`,
            );
        }
    };
    switch (opcode) {
        case Opcode.List: {
            if (body.length < LIST_PREFIX_LEN || (body.length - LIST_PREFIX_LEN) % LIST_ENTRY_LEN !== 0) {
                throw new ResponseError(
                    `a LIST page is ${LIST_PREFIX_LEN} bytes plus whole ${LIST_ENTRY_LEN}-byte entries, ` +
                        `not ${body.length}.`,
                );
            }
            const count = (body.length - LIST_PREFIX_LEN) / LIST_ENTRY_LEN;
            const entries: CatalogEntry[] = [];
            for (let i = 0; i < count; i++) entries.push(decodeEntry(body, LIST_PREFIX_LEN + i * LIST_ENTRY_LEN));
            return {
                opcode: Opcode.List,
                body: {
                    storeId: hex(body.subarray(0, 16)),
                    commitSequence: view.getBigUint64(16, true),
                    entries,
                    more,
                },
            };
        }
        case Opcode.Status: {
            expect(24);
            const state = body[0];
            if (state !== ObjectState.Absent && state !== ObjectState.Committed && state !== ObjectState.Superseded) {
                throw new ResponseError(`a STATUS response names state ${state}; §3.4 defines three.`);
            }
            return {
                opcode: Opcode.Status,
                body: {
                    state,
                    headRevision: view.getBigUint64(4, true),
                    headPayloadLength: view.getBigUint64(12, true),
                    headPayloadCrc32: view.getUint32(20, true),
                },
            };
        }
        case Opcode.Get:
            expect(24);
            return {
                opcode: Opcode.Get,
                body: {
                    revisionServed: view.getBigUint64(0, true),
                    payloadLength: view.getBigUint64(8, true),
                    payloadCrc32: view.getUint32(16, true),
                },
            };
        case Opcode.Put:
            expect(32);
            return {
                opcode: Opcode.Put,
                body: {
                    objectId: view.getBigUint64(0, true),
                    revision: view.getBigUint64(8, true),
                    payloadLength: view.getBigUint64(16, true),
                    payloadCrc32: view.getUint32(24, true),
                },
            };
        case Opcode.Remove:
            expect(8);
            return { opcode: Opcode.Remove, body: { commitSequence: view.getBigUint64(0, true) } };
        case Opcode.Cancel:
            expect(1);
            // §3.8: `0` cancelled, `1` no such transfer. Anything else is a third answer to a
            // two-valued question, and reading it as "cancelled" would be a guess.
            if (body[0] > 1) throw new ResponseError(`a CANCEL response says ${body[0]}; §3.8 defines 0 and 1.`);
            return { opcode: Opcode.Cancel, body: { cancelled: body[0] === 0 } };
        case Opcode.Arm:
            expect(16);
            return {
                opcode: Opcode.Arm,
                body: {
                    rollbackObjectId: view.getBigUint64(0, true),
                    commitSequence: view.getBigUint64(8, true),
                },
            };
        default:
            throw new ResponseError(`the device answered with ${opcodeName(opcode)}, which this client never sends.`);
    }
}

function decodeEntry(body: Uint8Array, at: number): CatalogEntry {
    const view = viewOf(body);
    const nameLength = Math.min(body[at + 32], NAME_CAPACITY);
    return {
        objectId: view.getBigUint64(at, true),
        revision: view.getBigUint64(at + 8, true),
        payloadLength: view.getBigUint64(at + 16, true),
        payloadCrc32: view.getUint32(at + 24, true),
        kind: view.getUint16(at + 28, true),
        flags: view.getUint16(at + 30, true),
        // Lenient where the request decoder is strict, and on purpose: a name is the one field a
        // client only ever *renders*. Refusing a whole catalog page because one entry's name is not
        // valid UTF-8 would hide every other object on the card behind a cosmetic fault.
        displayName: new TextDecoder().decode(body.subarray(at + 36, at + 36 + nameLength)),
    };
}

// --- response encoders (the mock device's side) -----------------------------------

/** §3.3's page: the 24-byte prefix, then `entries`. */
export function encodeListResponse(requestId: number, page: ListPage): Uint8Array {
    const body = new Uint8Array(LIST_PREFIX_LEN + page.entries.length * LIST_ENTRY_LEN);
    body.set(unhex(page.storeId, 16), 0);
    new DataView(body.buffer).setBigUint64(16, page.commitSequence, true);
    page.entries.forEach((entry, i) => encodeEntry(body, LIST_PREFIX_LEN + i * LIST_ENTRY_LEN, entry));
    return encodeControl(Opcode.List, Flags.Response | (page.more ? Flags.More : 0), requestId, body);
}

function encodeEntry(body: Uint8Array, at: number, entry: CatalogEntry): void {
    const view = new DataView(body.buffer, body.byteOffset, body.byteLength);
    view.setBigUint64(at, entry.objectId, true);
    view.setBigUint64(at + 8, entry.revision, true);
    view.setBigUint64(at + 16, entry.payloadLength, true);
    view.setUint32(at + 24, entry.payloadCrc32, true);
    view.setUint16(at + 28, entry.kind, true);
    view.setUint16(at + 30, entry.flags, true);
    encodeName(body, at + 32, at + 36, entry.displayName);
}

/** §3.4's 24-byte response. */
export function encodeStatusResponse(requestId: number, answer: StatusResponse): Uint8Array {
    const body = new Uint8Array(24);
    const view = new DataView(body.buffer);
    body[0] = answer.state;
    view.setBigUint64(4, answer.headRevision, true);
    view.setBigUint64(12, answer.headPayloadLength, true);
    view.setUint32(20, answer.headPayloadCrc32, true);
    return encodeControl(Opcode.Status, Flags.Response, requestId, body);
}

/** §3.5's 24-byte response. */
export function encodeGetResponse(requestId: number, answer: GetResponse): Uint8Array {
    const body = new Uint8Array(24);
    const view = new DataView(body.buffer);
    view.setBigUint64(0, answer.revisionServed, true);
    view.setBigUint64(8, answer.payloadLength, true);
    view.setUint32(16, answer.payloadCrc32, true);
    return encodeControl(Opcode.Get, Flags.Response, requestId, body);
}

/** §3.6's 32-byte response. */
export function encodePutResponse(requestId: number, answer: PutResponse): Uint8Array {
    const body = new Uint8Array(32);
    const view = new DataView(body.buffer);
    view.setBigUint64(0, answer.objectId, true);
    view.setBigUint64(8, answer.revision, true);
    view.setBigUint64(16, answer.payloadLength, true);
    view.setUint32(24, answer.payloadCrc32, true);
    return encodeControl(Opcode.Put, Flags.Response, requestId, body);
}

/** §3.7's 8-byte response: the new catalog commit sequence, and nothing else. */
export function encodeRemoveResponse(requestId: number, commitSequence: bigint): Uint8Array {
    const body = new Uint8Array(8);
    new DataView(body.buffer).setBigUint64(0, commitSequence, true);
    return encodeControl(Opcode.Remove, Flags.Response, requestId, body);
}

/** §3.8's 1-byte response: `0` cancelled, `1` no such transfer. */
export function encodeCancelResponse(requestId: number, cancelled: boolean): Uint8Array {
    return encodeControl(Opcode.Cancel, Flags.Response, requestId, new Uint8Array([cancelled ? 0 : 1]));
}

/** §4's 16-byte response. */
export function encodeArmResponse(requestId: number, answer: ArmResponse): Uint8Array {
    const body = new Uint8Array(16);
    const view = new DataView(body.buffer);
    view.setBigUint64(0, answer.rollbackObjectId, true);
    view.setBigUint64(8, answer.commitSequence, true);
    return encodeControl(Opcode.Arm, Flags.Response, requestId, body);
}

// --- the stream channel (§3.8) -----------------------------------------------------

/** §3.8's 16-byte stream frame. */
export interface StreamFrame {
    readonly transferRequestId: number;
    readonly offset: bigint;
    readonly payloadLength: number;
}

/** One stream record: the 16-byte frame immediately followed by exactly `payload` bytes. */
export function encodeStreamRecord(transferRequestId: number, offset: bigint, payload: Uint8Array): Uint8Array {
    if (payload.length === 0 || payload.length > 0xffff) {
        throw new RangeError(`a stream record carries 1..=65535 payload bytes, this one has ${payload.length}.`);
    }
    const out = new Uint8Array(STREAM_HEADER_LEN + payload.length);
    const view = new DataView(out.buffer);
    view.setUint32(0, transferRequestId >>> 0, true);
    view.setBigUint64(4, offset, true);
    view.setUint16(12, payload.length, true);
    out.set(payload, STREAM_HEADER_LEN);
    return out;
}

/**
 * Split one stream record into its frame and its payload, or `null`.
 *
 * `null` is §3.8's "a zero length, a length disagreeing with the record", plus a nonzero reserved
 * field. A record this cannot split names no offset the receiver can trust, so there is nothing to
 * answer with beyond terminating the transfer it claims to belong to.
 */
/**
 * Why a §3.8 record could not be split — the three distinguishable ways it can be malformed.
 *
 * `splitStreamRecord` answers `null` for all three, which is all a caller needs: §3.8 gives a
 * malformed stream record no answer of its own, it terminates the transfer. A *test* needs more,
 * because three fixtures that each assert `toBeNull()` pass identically whether the codec refused
 * them for the stated reason or for any other one — which is how a rejection can be right by
 * accident. {@link streamRecordFault} is that discrimination, and it exists for the vector suite.
 */
export type StreamRecordFault =
    /** Shorter than the 16-byte frame: there is not even a header to read. */
    | "short"
    /** Bytes 14..16 are §3.8's reserved zero and are not zero. */
    | "reservedBits"
    /** A zero payload length, which §3.8 forbids outright. */
    | "zeroLength"
    /** The declared payload length disagrees with the record that carried it. */
    | "lengthMismatch";

/** Which of {@link StreamRecordFault} a record trips, or `null` when it splits cleanly. */
export function streamRecordFault(record: Uint8Array): StreamRecordFault | null {
    if (record.length < STREAM_HEADER_LEN) return "short";
    const view = viewOf(record);
    if (view.getUint16(14, true) !== 0) return "reservedBits";
    const payloadLength = view.getUint16(12, true);
    if (payloadLength === 0) return "zeroLength";
    if (record.length !== STREAM_HEADER_LEN + payloadLength) return "lengthMismatch";
    return null;
}

export function splitStreamRecord(record: Uint8Array): { frame: StreamFrame; payload: Uint8Array } | null {
    if (streamRecordFault(record) !== null) return null;
    const view = viewOf(record);
    const payloadLength = view.getUint16(12, true);
    return {
        frame: {
            transferRequestId: view.getUint32(0, true),
            offset: view.getBigUint64(4, true),
            payloadLength,
        },
        payload: record.subarray(STREAM_HEADER_LEN),
    };
}

// --- shared helpers ------------------------------------------------------------------

/**
 * A `DataView` over exactly the bytes of `data` — `subarray` keeps the parent buffer, so the offset
 * and length have to be carried explicitly or every field read is off by the slice.
 *
 * Exported because `objects.ts` and `device/route.ts` read object *payloads* with it: the payload
 * codecs are not wire codecs, but they parse the same little-endian layouts and there is no reason
 * for a second copy of this one line.
 */
export function viewOf(data: Uint8Array): DataView {
    return new DataView(data.buffer, data.byteOffset, data.byteLength);
}

/**
 * A `bigint` as a JS `number`, refusing anything `Number` would round.
 *
 * The narrowing exists for one honest case: a length that has to become a `Uint8Array` size or a
 * progress figure. A payload above 2^53 bytes is not a number this client can carry, and it is also
 * not a card that exists, so refusing is both correct and unreachable.
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

/** The inverse, for an encoder handed a `StoreId` as text. */
export function unhex(text: string, bytes: number): Uint8Array {
    if (text.length !== bytes * 2) throw new RangeError(`expected ${bytes * 2} hex digits, got ${text.length}.`);
    const out = new Uint8Array(bytes);
    for (let i = 0; i < bytes; i++) {
        const byte = Number.parseInt(text.slice(i * 2, i * 2 + 2), 16);
        if (Number.isNaN(byte)) throw new RangeError(`"${text}" is not hex.`);
        out[i] = byte;
    }
    return out;
}

function isZero(bytes: Uint8Array): boolean {
    for (const byte of bytes) if (byte !== 0) return false;
    return true;
}
