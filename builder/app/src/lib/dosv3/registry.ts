/**
 * Device_Object_Registries_v2.md: the stable numeric assignments and the bounded domain schemas.
 *
 * The wire contract owns message layouts; this file owns what a kind *is* — which operations it may
 * advertise, which metadata fields its three schemas carry, and how large each of those envelopes
 * may get. Keeping it separate is the registry's own rule: "a domain changes only its own section
 * and the shared vectors; it does not broaden a common repository or transport interface."
 */

/** ObjectKind, registry §1. `0` is never encoded and `5` must not be advertised or encoded. */
export const OBJECT_KIND = {
    route: 1,
    trip: 2,
    ride: 3,
    weather: 4,
    volumeManifest: 6,
    updatePackage: 7,
} as const;
export type ObjectKindName = keyof typeof OBJECT_KIND;
export type ObjectKind = (typeof OBJECT_KIND)[ObjectKindName];

export const OBJECT_KIND_NAME: ReadonlyMap<number, ObjectKindName> = new Map(
    (Object.entries(OBJECT_KIND) as [ObjectKindName, ObjectKind][]).map(([name, value]) => [value, name]),
);

/** DraftPartKind, registry §2. */
export const DRAFT_PART_KIND = { standaloneMapBlob: 1, mapShard: 2, terrainBlob: 3, volumeIndex: 4 } as const;
export type DraftPartKindName = keyof typeof DRAFT_PART_KIND;
export type DraftPartKind = (typeof DRAFT_PART_KIND)[DraftPartKindName];

export const DRAFT_PART_KIND_NAME: ReadonlyMap<number, DraftPartKindName> = new Map(
    (Object.entries(DRAFT_PART_KIND) as [DraftPartKindName, DraftPartKind][]).map(([name, value]) => [value, name]),
);

/** Subject-entry operation flags, wire §5. */
export const SUBJECT_OP = {
    put: 1 << 0,
    get: 1 << 1,
    delete: 1 << 2,
    setMetadata: 1 << 3,
    resumableUpload: 1 << 4,
    resumableDownload: 1 << 5,
    draftFinalize: 1 << 6,
} as const;
export const SUBJECT_OP_MASK = 0x7f;

/** Subject-entry policy flags, wire §5. */
export const SUBJECT_POLICY = {
    usbRecommended: 1 << 0,
    externalPowerRequired: 1 << 1,
    authenticatedPrincipalRequired: 1 << 2,
    fixedSingleton: 1 << 3,
} as const;
export const SUBJECT_POLICY_MASK = 0x0f;

/**
 * Registry §1's lifecycle table, as the mask of operation bits a kind may advertise. "A `no` is
 * normative: a device that advertises it is nonconforming", so a subject entry claiming a bit
 * outside its row is rejected rather than tolerated. The two resumable bits are device policy and
 * are permitted for every kind.
 */
const RESUMABLE = SUBJECT_OP.resumableUpload | SUBJECT_OP.resumableDownload;
export const PERMITTED_SUBJECT_OPS: Readonly<Record<ObjectKindName, number>> = {
    route: SUBJECT_OP.put | SUBJECT_OP.get | SUBJECT_OP.delete | SUBJECT_OP.setMetadata | RESUMABLE,
    trip: SUBJECT_OP.put | SUBJECT_OP.get | SUBJECT_OP.delete | RESUMABLE,
    ride: SUBJECT_OP.get | SUBJECT_OP.delete | RESUMABLE,
    weather: SUBJECT_OP.put | SUBJECT_OP.get | SUBJECT_OP.delete | RESUMABLE,
    volumeManifest: SUBJECT_OP.get | SUBJECT_OP.delete | SUBJECT_OP.setMetadata | SUBJECT_OP.draftFinalize | RESUMABLE,
    updatePackage: SUBJECT_OP.put | SUBJECT_OP.get | SUBJECT_OP.delete | RESUMABLE,
};

/** A draft-part subject advertises put and optionally resumable upload, and nothing else (§5). */
export const PERMITTED_DRAFT_PART_OPS = SUBJECT_OP.put | SUBJECT_OP.resumableUpload;

/** The three registered schema versions, registry §4. They are constants, not a negotiation. */
export const SCHEMA_VERSION = { put: 1, patch: 128, catalog: 64 } as const;
export type SchemaRole = keyof typeof SCHEMA_VERSION;

export const SCHEMA_ROLE_OF_VERSION: ReadonlyMap<number, SchemaRole> = new Map([
    [SCHEMA_VERSION.put, "put"],
    [SCHEMA_VERSION.patch, "patch"],
    [SCHEMA_VERSION.catalog, "catalog"],
]);

/** Envelope ceilings, wire §2.2. Put and patch share one; the catalog projection has its own. */
export const ENVELOPE_CEILING: Readonly<Record<SchemaRole, number>> = { put: 128, patch: 128, catalog: 96 };
export const ENVELOPE_HEADER_BYTES = 8;

export type MetadataFieldType =
    | { readonly kind: "u8" }
    | { readonly kind: "u16" }
    | { readonly kind: "u32" }
    | { readonly kind: "u64" }
    | { readonly kind: "i32" }
    | { readonly kind: "i64" }
    | { readonly kind: "bool" }
    | { readonly kind: "text"; readonly min: number; readonly max: number }
    | { readonly kind: "bytes"; readonly exact: number };

/**
 * A registered value range. The registry states these in prose next to each field ("range
 * -900,000,000 through 900,000,000", "nonzero and at most 100,000", "never `0`, day `1`, … two
 * months `5`"), and they are as much part of the schema as the field's width is: a decoder that
 * accepts a latitude of 2 billion has validated nothing the registry asked for.
 *
 * `enumerated` separates the two kinds, because they fail differently: a value outside a registered
 * enumeration is `unknownEnum`, and a continuous quantity outside its bounds is `invalidCombination`.
 */
export interface MetadataBounds {
    readonly min: bigint;
    readonly max: bigint;
    readonly enumerated: boolean;
}

const enumerated = (min: number, max: number): MetadataBounds => ({
    min: BigInt(min),
    max: BigInt(max),
    enumerated: true,
});
const bounded = (min: number, max: number): MetadataBounds => ({
    min: BigInt(min),
    max: BigInt(max),
    enumerated: false,
});

/** Retention, registry §4.1: never `0`, day `1`, week `2`, two weeks `3`, month `4`, two months `5`. */
const RETENTION = enumerated(0, 5);
/** Update states, registry §4.3: VerifiedReady `1` through failed `6`. */
const UPDATE_STATE = enumerated(1, 6);
/** The weather request context's coordinate and coverage ranges, registry §3. */
export const WEATHER_LATITUDE = bounded(-900_000_000, 900_000_000);
export const WEATHER_LONGITUDE = bounded(-1_800_000_000, 1_800_000_000);
export const WEATHER_RADIUS_METRES = bounded(1, 100_000);

export interface MetadataFieldSpec {
    readonly name: string;
    readonly tag: number;
    readonly type: MetadataFieldType;
    readonly required: boolean;
    readonly bounds?: MetadataBounds;
}

const u8 = { kind: "u8" } as const;
const u16 = { kind: "u16" } as const;
const u32 = { kind: "u32" } as const;
const u64 = { kind: "u64" } as const;
const i32 = { kind: "i32" } as const;
const i64 = { kind: "i64" } as const;
const bool = { kind: "bool" } as const;
const text = (min: number, max: number): MetadataFieldType => ({ kind: "text", min, max });

export interface MetadataSchema {
    readonly kind: ObjectKindName;
    readonly role: SchemaRole;
    /** The registered maximum encoded envelope length, registry §4. Includes the eight-byte header. */
    readonly maxBytes: number;
    readonly fields: readonly MetadataFieldSpec[];
}

function schema(
    kind: ObjectKindName,
    role: SchemaRole,
    maxBytes: number,
    fields: readonly MetadataFieldSpec[],
): MetadataSchema {
    return { kind, role, maxBytes, fields };
}

const field = (
    name: string,
    tag: number,
    type: MetadataFieldType,
    required: boolean,
    bounds?: MetadataBounds,
): MetadataFieldSpec => ({ name, tag, type, required, bounds });

/** Registry §4.1, Put v1. Trip, ride, volume-manifest and update-package Put v1 have zero fields. */
const PUT_SCHEMAS: Readonly<Record<ObjectKindName, MetadataSchema>> = {
    route: schema("route", "put", 13, [field("retention", 0x8001, u8, true, RETENTION)]),
    trip: schema("trip", "put", 8, []),
    ride: schema("ride", "put", 8, []),
    weather: schema("weather", "put", 68, [
        field("weatherRequestId", 0x8001, u64, true),
        field("centreLatitude", 0x8002, i32, true, WEATHER_LATITUDE),
        field("centreLongitude", 0x8003, i32, true, WEATHER_LONGITUDE),
        field("radiusMetres", 0x8004, u32, true, WEATHER_RADIUS_METRES),
        field("issuedUtc", 0x8005, i64, true),
        field("validUntilUtc", 0x8006, i64, true),
    ]),
    volumeManifest: schema("volumeManifest", "put", 8, []),
    updatePackage: schema("updatePackage", "put", 8, []),
};

/** Registry §4.2, SetMetadata v128. Kinds absent here reject SetMetadata as unsupported. */
const PATCH_SCHEMAS: Partial<Readonly<Record<ObjectKindName, MetadataSchema>>> = {
    route: schema("route", "patch", 70, [
        field("retention", 0x8001, u8, false, RETENTION),
        field("selected", 0x8002, bool, false),
        field("displayName", 0x8003, text(1, 48), false),
    ]),
    volumeManifest: schema("volumeManifest", "patch", 13, [field("selected", 0x8001, bool, false)]),
};

/** Registry §4.3, catalog projection v64. */
const CATALOG_SCHEMAS: Readonly<Record<ObjectKindName, MetadataSchema>> = {
    route: schema("route", "catalog", 82, [
        field("displayName", 0x8001, text(1, 48), true),
        field("retention", 0x8002, u8, true, RETENTION),
        field("selected", 0x0003, bool, false),
        field("trustedCreationUtc", 0x0004, i64, false),
    ]),
    trip: schema("trip", "catalog", 66, [
        field("displayName", 0x8001, text(1, 48), true),
        field("stageCount", 0x8002, u16, true),
    ]),
    ride: schema("ride", "catalog", 41, [
        field("startUtc", 0x8001, i64, true),
        field("durationSeconds", 0x8002, u32, true),
        field("distanceMetres", 0x8003, u32, true),
        field("imported", 0x8004, bool, true),
    ]),
    weather: schema("weather", "catalog", 44, [
        field("weatherRequestId", 0x8001, u64, true),
        field("issuedUtc", 0x8002, i64, true),
        field("validUntilUtc", 0x8003, i64, true),
    ]),
    volumeManifest: schema("volumeManifest", "catalog", 55, [
        field("displayName", 0x8001, text(1, 32), true),
        field("selected", 0x8002, bool, true),
        field("referencedPartCount", 0x8003, u16, true),
    ]),
    updatePackage: schema("updatePackage", "catalog", 77, [
        field("semanticVersion", 0x8001, text(1, 24), true),
        field("state", 0x8002, u8, true, UPDATE_STATE),
        field("imageDigest", 0x8003, { kind: "bytes", exact: 32 }, true),
    ]),
};

/** The schema for one (kind, role), or undefined when the kind supports no such operation. */
export function metadataSchema(kind: ObjectKindName, role: SchemaRole): MetadataSchema | undefined {
    if (role === "put") return PUT_SCHEMAS[kind];
    if (role === "patch") return PATCH_SCHEMAS[kind];
    return CATALOG_SCHEMAS[kind];
}

/** Registry §6: the ObjectKind-scoped semanticValidation detail tables. */
export const SEMANTIC_DETAILS: Readonly<Record<ObjectKindName, Readonly<Record<string, number>>>> = {
    route: { invalidRouteFormat: 1 },
    trip: { invalidTripFormat: 1, duplicateRouteReference: 2, missingTripRoute: 3 },
    ride: { invalidRideFormat: 1, alreadyImported: 2 },
    weather: {
        supersededNotUseful: 1,
        coverageMismatch: 2,
        staleBundle: 3,
        payloadFactsMismatch: 4,
        requestMismatch: 5,
    },
    volumeManifest: {
        invalidManifest: 1,
        missingDraftPart: 2,
        foreignDraftPart: 3,
        duplicateDraftReference: 4,
        duplicateDraftPart: 5,
        draftNotOpen: 6,
        draftIncomplete: 7,
    },
    updatePackage: {
        invalidSignature: 1,
        digestMismatch: 2,
        wrongTarget: 3,
        downgradeDenied: 4,
        packageTooLarge: 5,
        unsafePowerState: 6,
        unsafeRuntimeState: 7,
        notVerifiedReady: 8,
    },
};

const SEMANTIC_DETAIL_NAMES: ReadonlyMap<ObjectKindName, ReadonlyMap<number, string>> = new Map(
    (Object.keys(SEMANTIC_DETAILS) as ObjectKindName[]).map((kind) => [
        kind,
        new Map(Object.entries(SEMANTIC_DETAILS[kind]).map(([name, value]) => [value, name])),
    ]),
);

/** The name of a domain semantic detail, or "unknown" for a code this registry has not allocated. */
export function semanticDetailName(kind: ObjectKindName, detail: number): string {
    if (detail === 0) return "none";
    return SEMANTIC_DETAIL_NAMES.get(kind)?.get(detail) ?? "unknown";
}
