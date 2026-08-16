/**
 * Device Object Protocol v3 identities (Device_Object_System_v2.md §identity model).
 *
 * The system contract's first acceptance criterion is that the identifiers are *mechanically*
 * distinct in every language: a `StoreId` may not be passed where an `OperationId` is wanted, and a
 * `Revision` may not be passed where a `LogicalObjectId` is. TypeScript has no nominal types, so
 * each one is a branded alias — the brand exists only in the type system and costs nothing at
 * runtime, but `storeId(bytes)` and `operationId(bytes)` produce values the compiler refuses to
 * cross-assign.
 *
 * The 64-bit identities are `bigint`, never `number`. The wire contract §1 is explicit: a codec
 * MUST carry the full unsigned 64-bit range and one that silently truncates to 32 bits is
 * nonconforming. `Number` loses integers above 2^53, which is inside the field width even though it
 * is above today's ResourceLimits bound.
 */

declare const idBrand: unique symbol;

type Branded<T, B extends string> = T & { readonly [idBrand]: B };

/** 128-bit store identity, created when an OBC2 store is initialized. */
export type StoreId = Branded<Uint8Array, "StoreId">;
/** 128-bit idempotency key, generated before any mutating operation. */
export type OperationId = Branded<Uint8Array, "OperationId">;
/** 128-bit opaque reference minted for a sealed draft part. Carries no decodable structure. */
export type DraftPartRef = Branded<Uint8Array, "DraftPartRef">;
/** 128-bit opaque catalog/draft page cursor. Its codec is normative but it is opaque to callers. */
export type PageCursor = Branded<Uint8Array, "PageCursor">;
/** 128-bit opaque device serial. Not a StoreId: replacing the card changes one and not the other. */
export type DeviceSerial = Branded<Uint8Array, "DeviceSerial">;

/** Opaque unsigned 64-bit identity inside one object kind and StoreId. No sentinel values. */
export type LogicalObjectId = Branded<bigint, "LogicalObjectId">;
/** Unsigned 64-bit monotonically increasing repository revision. */
export type Revision = Branded<bigint, "Revision">;
/** Unsigned 64-bit monotonic draft-parent revision. Not a repository Revision. */
export type DraftRevision = Branded<bigint, "DraftRevision">;
/** Unsigned 64-bit weather domain request identity. Never a control RequestId, never a logical ID. */
export type WeatherRequestId = Branded<bigint, "WeatherRequestId">;
/** Unsigned 64-bit draft part key, unique with its DraftPartKind inside one parent. */
export type PartKey = Branded<bigint, "PartKey">;

/** Nonzero unsigned 32-bit ephemeral stream capability, scoped to one BLE or USB owner. */
export type SessionId = Branded<number, "SessionId">;
/** Nonzero unsigned 32-bit control request correlation. Neither SessionId nor OperationId. */
export type RequestId = Branded<number, "RequestId">;

/** The width of every 128-bit identity in this contract. */
export const IDENTITY_BYTES = 16;

/** The v3.0 ResourceLimits bound on every u64 length, offset, and byte count. */
export const U64_V30_BOUND = 0xffff_ffffn;

const U64_MAX = (1n << 64n) - 1n;

function checked16(bytes: Uint8Array, what: string): Uint8Array {
    if (bytes.length !== IDENTITY_BYTES) throw new RangeError(`${what} is ${IDENTITY_BYTES} bytes, got ${bytes.length}`);
    return Uint8Array.from(bytes);
}

function checkedU64(value: bigint, what: string): bigint {
    if (value < 0n || value > U64_MAX) throw new RangeError(`${what} is out of the unsigned 64-bit range`);
    return value;
}

export const storeId = (bytes: Uint8Array): StoreId => checked16(bytes, "StoreId") as StoreId;
export const operationId = (bytes: Uint8Array): OperationId => checked16(bytes, "OperationId") as OperationId;
export const draftPartRef = (bytes: Uint8Array): DraftPartRef => checked16(bytes, "DraftPartRef") as DraftPartRef;
export const pageCursor = (bytes: Uint8Array): PageCursor => checked16(bytes, "PageCursor") as PageCursor;
export const deviceSerial = (bytes: Uint8Array): DeviceSerial => checked16(bytes, "DeviceSerial") as DeviceSerial;

export const logicalObjectId = (value: bigint): LogicalObjectId =>
    checkedU64(value, "LogicalObjectId") as LogicalObjectId;
export const revision = (value: bigint): Revision => checkedU64(value, "Revision") as Revision;
export const draftRevision = (value: bigint): DraftRevision => checkedU64(value, "DraftRevision") as DraftRevision;
export const weatherRequestId = (value: bigint): WeatherRequestId =>
    checkedU64(value, "WeatherRequestId") as WeatherRequestId;
export const partKey = (value: bigint): PartKey => checkedU64(value, "PartKey") as PartKey;

function checkedNonzeroU32(value: number, what: string): number {
    if (!Number.isInteger(value) || value <= 0 || value > 0xffff_ffff) {
        throw new RangeError(`${what} is a nonzero unsigned 32-bit value`);
    }
    return value;
}

export const sessionId = (value: number): SessionId => checkedNonzeroU32(value, "SessionId") as SessionId;
export const requestId = (value: number): RequestId => checkedNonzeroU32(value, "RequestId") as RequestId;

/** True when a u64 wire value is inside the bound v3.0 ResourceLimits advertises (§1). */
export const withinV30Bound = (value: bigint): boolean => value >= 0n && value <= U64_V30_BOUND;

/** Byte equality for the 128-bit identities. They are opaque bytes; no field reordering applies. */
export function identityEquals(a: Uint8Array, b: Uint8Array): boolean {
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
    return true;
}

/** True when every byte of a 128-bit identity is zero — the "inactive"/"unknowable" encoding. */
export function identityIsZero(a: Uint8Array): boolean {
    for (let i = 0; i < a.length; i++) if (a[i] !== 0) return false;
    return true;
}

export const ZERO_IDENTITY: Uint8Array = new Uint8Array(IDENTITY_BYTES);
