/**
 * The properties the shared vectors deliberately do *not* carry as bytes.
 *
 * `Device_Object_Vectors_v2.md` §2.2 is explicit about one of them: the schema ceilings are
 * asserted arithmetically rather than as fixtures, "because no legal envelope reaches one, so a
 * ceiling vector would be a fixture a conforming decoder must reject." The rest are the same shape
 * — the identity model, the 64-bit field width, the validation *order*, the encoder's refusal to
 * truncate, and totality — facts about the codec that a single byte vector cannot state.
 *
 * Several of these pin boundaries a shared negative fixture will land on later. When it does, the
 * manifest-driven suite picks it up and these keep the same behaviour honest in the meantime.
 */

import { describe, expect, it } from "vitest";

import {
    CATEGORY,
    CRC32_CHECK_INPUT,
    CRC32_CHECK_VALUE,
    CATALOG_ENTRY_PREFIX_BYTES,
    CATALOG_METADATA_LIMIT,
    CATALOG_PAGE_PREFIX_BYTES,
    ENVELOPE_HEADER_BYTES,
    GUIDANCE,
    IDENTITY_BYTES,
    MAX_CONTROL_PAYLOAD,
    METADATA_ENVELOPE_LIMIT,
    MIN_CONTROL_FRAME,
    MIN_STREAM_FRAME,
    MAX_STREAM_FRAME_BYTES,
    OBJECT_KIND,
    OPCODE,
    PRESENCE,
    STREAM_DIRECTION,
    STREAM_FLAG,
    U64_V30_BOUND,
    bytesToHex,
    crc32,
    decodeCapabilities,
    decodeConfigBlock,
    decodeControlFrame,
    decodeErrorBody,
    decodeMetadataEnvelope,
    decodeStreamFrame,
    decodeSubjectEntry,
    detailName,
    encodeControlFrame,
    encodeStreamFrame,
    errorText,
    hexToBytes,
    isRetainedTerminalReplay,
    logicalObjectId,
    metadataSchema,
    negotiateFrameLimit,
    operationId,
    requestId,
    sessionId,
    storeId,
    unwrap,
    withinV30Bound,
    type DosResult,
    type ObjectKindName,
    type OperationId,
    type SchemaRole,
    type StoreId,
} from "./index";

// ------------------------------------------------------------------------------- tiny builders

/** Assembles a control record without going through the encoder under test. */
function controlRecord(opcode: number, flags: number, requestIdValue: number, payload: Uint8Array): Uint8Array {
    const bytes = new Uint8Array(16 + payload.length);
    const view = new DataView(bytes.buffer);
    bytes.set([0x4f, 0x42, 0x43, 0x50], 0);
    bytes[4] = 3;
    bytes[5] = 0;
    view.setUint16(6, opcode, true);
    view.setUint16(8, flags, true);
    view.setUint16(10, payload.length, true);
    view.setUint32(12, requestIdValue, true);
    bytes.set(payload, 16);
    return bytes;
}

/** Assembles a metadata envelope from raw `(tag, value)` pairs, canonical or not. */
function envelope(
    schemaId: number,
    version: number,
    fields: readonly { tag: number; value: Uint8Array }[],
    overrides: { encodedFieldBytes?: number; fieldCount?: number; flags?: number } = {},
): Uint8Array {
    const body: number[] = [];
    for (const field of fields) {
        body.push(field.tag & 0xff, field.tag >> 8, field.value.length & 0xff, field.value.length >> 8);
        body.push(...field.value);
    }
    const declared = overrides.encodedFieldBytes ?? body.length;
    const count = overrides.fieldCount ?? fields.length;
    const bytes = new Uint8Array(ENVELOPE_HEADER_BYTES + Math.max(body.length, 0));
    const view = new DataView(bytes.buffer);
    view.setUint16(0, schemaId, true);
    bytes[2] = version;
    bytes[3] = overrides.flags ?? 0;
    view.setUint16(4, declared, true);
    view.setUint16(6, count, true);
    bytes.set(body, ENVELOPE_HEADER_BYTES);
    return bytes;
}

const int = (value: number, width: number): Uint8Array => {
    const bytes = new Uint8Array(width);
    const view = new DataView(bytes.buffer);
    if (width === 1) view.setInt8(0, value);
    else if (width === 4) view.setInt32(0, value, true);
    else view.setBigInt64(0, BigInt(value), true);
    return bytes;
};

/** A well-formed weather Put envelope with one field swapped for a caller-supplied value. */
function weatherPut(overrides: Partial<Record<number, Uint8Array>> = {}): Uint8Array {
    const base: Record<number, Uint8Array> = {
        0x8001: int(7, 8),
        0x8002: int(48_000_000, 4),
        0x8003: int(7_700_000, 4),
        0x8004: int(50_000, 4),
        0x8005: int(1_700_000_000, 8),
        0x8006: int(1_700_086_400, 8),
    };
    const fields = Object.keys(base)
        .map(Number)
        .sort((a, b) => a - b)
        .map((tag) => ({ tag, value: overrides[tag] ?? base[tag] }));
    return envelope(OBJECT_KIND.weather, 1, fields);
}

const failure = (result: DosResult<unknown>): { category: string; detail: string } => {
    if (result.ok) throw new Error("expected a rejection, and the codec accepted the bytes");
    return { category: result.error.category, detail: result.error.detail };
};

// --------------------------------------------------------------------------------- the suites

describe("the derived control-frame floor (§1, §2.2)", () => {
    it("is the sum of the two schema ceilings plus the header", () => {
        const catalogCeiling = CATALOG_PAGE_PREFIX_BYTES + CATALOG_ENTRY_PREFIX_BYTES + CATALOG_METADATA_LIMIT;
        const startUploadCeiling = 48 + METADATA_ENVELOPE_LIMIT;
        expect(catalogCeiling).toBe(176);
        expect(startUploadCeiling).toBe(176);
        expect(Math.max(catalogCeiling, startUploadCeiling) + 16).toBe(MIN_CONTROL_FRAME);
    });

    it("is comfortably above what any registered schema can actually produce", () => {
        // §2.2: the largest producible catalog entry is route's 44 + 36 + 82 and the largest
        // producible StartUpload is weather's 48 + 68. Neither ceiling is reachable.
        expect(CATALOG_PAGE_PREFIX_BYTES + CATALOG_ENTRY_PREFIX_BYTES + (metadataSchema("route", "catalog")?.maxBytes ?? 0)).toBe(162);
        expect(48 + (metadataSchema("weather", "put")?.maxBytes ?? 0)).toBe(116);
    });
});

describe("CRC-32/IEEE (§1)", () => {
    it("matches the parameterization's check value", () => {
        expect(crc32(new TextEncoder().encode(CRC32_CHECK_INPUT))).toBe(CRC32_CHECK_VALUE);
    });

    it("folds segments the same way as one contiguous buffer", () => {
        const whole = Uint8Array.from({ length: 64 }, (_, i) => i * 7);
        expect(crc32(whole.subarray(0, 20), whole.subarray(20))).toBe(crc32(whole));
    });
});

describe("the metadata validation order (§2.2)", () => {
    const decode = (bytes: Uint8Array): DosResult<unknown> => decodeMetadataEnvelope(bytes, { mutating: true });

    /**
     * Each of these breaks two rules at once, and the frozen order decides which one is reported.
     * They are the cases an independent implementation gets wrong first, because every check looks
     * equally reasonable in isolation.
     */
    it("reports out-of-order fields before an unregistered schema version", () => {
        expect(failure(decode(hexToBytes("010063000a00020002800100000180010000")))).toEqual({
            category: "invalidDescriptor",
            detail: "outOfOrderField",
        });
    });

    it("reports a duplicate base tag before an unregistered schema id", () => {
        expect(failure(decode(hexToBytes("090040000a00020001800100000100010000")))).toEqual({
            category: "invalidDescriptor",
            detail: "duplicateField",
        });
    });

    it("reports a zero base tag before the common role ceiling", () => {
        // A catalog envelope declaring 89 field bytes is over the 88-byte ceiling *and* carries a
        // zero base tag. Canonical form comes first.
        const bytes = envelope(OBJECT_KIND.route, 64, [{ tag: 0x8000, value: int(0, 1) }], { encodedFieldBytes: 89 });
        expect(failure(decode(bytes))).toEqual({ category: "invalidDescriptor", detail: "noncanonicalMetadata" });
    });

    it("reports an unknown critical field before the per-kind maximum", () => {
        // 8 + 5 + 5 = 18 bytes against route Put's registered 13, and one of the fields is unknown.
        const bytes = envelope(OBJECT_KIND.route, 1, [
            { tag: 0x8001, value: int(2, 1) },
            { tag: 0x8055, value: int(9, 1) },
        ]);
        expect(bytes.length).toBeGreaterThan(metadataSchema("route", "put")?.maxBytes ?? 0);
        expect(failure(decode(bytes))).toEqual({ category: "invalidDescriptor", detail: "invalidCombination" });
    });

    it("reports the per-kind maximum once the fields themselves validate", () => {
        const bytes = envelope(OBJECT_KIND.ride, 64, [
            { tag: 0x8001, value: int(1_700_000_000, 8) },
            { tag: 0x8002, value: int(5_400, 4) },
            { tag: 0x8003, value: int(42_000, 4) },
            { tag: 0x8004, value: int(1, 1) },
            { tag: 0x0055, value: new Uint8Array(40) }, // noncritical: a projection may skip it
        ]);
        expect(failure(decodeMetadataEnvelope(bytes, { mutating: false }))).toEqual({
            category: "invalidDescriptor",
            detail: "nestedLength",
        });
    });
});

describe("registered value ranges are enforced (registry §3, §4)", () => {
    const put = (kind: number, fields: readonly { tag: number; value: Uint8Array }[]): DosResult<unknown> =>
        decodeMetadataEnvelope(envelope(kind, 1, fields), { mutating: true });

    it.each([
        [0, true],
        [5, true],
        [6, false],
        [255, false],
    ])("route retention %i", (value, valid) => {
        const result = put(OBJECT_KIND.route, [{ tag: 0x8001, value: int(value, 1) }]);
        expect(result.ok).toBe(valid);
        if (!result.ok) expect(result.error.detail).toBe("unknownEnum");
    });

    it.each([
        [0, false],
        [1, true],
        [6, true],
        [7, false],
    ])("update-package state %i", (value, valid) => {
        const bytes = envelope(OBJECT_KIND.updatePackage, 64, [
            { tag: 0x8001, value: new TextEncoder().encode("1.2.3") },
            { tag: 0x8002, value: int(value, 1) },
            { tag: 0x8003, value: new Uint8Array(32) },
        ]);
        const result = decodeMetadataEnvelope(bytes, { mutating: false });
        expect(result.ok).toBe(valid);
        if (!result.ok) expect(result.error.detail).toBe("unknownEnum");
    });

    it.each([
        ["latitude", 0x8002, 4, 900_000_000, 900_000_001],
        ["longitude", 0x8003, 4, 1_800_000_000, 1_800_000_001],
    ])("weather %s stops at its registered bound", (_name, tag, width, inside, outside) => {
        expect(decodeMetadataEnvelope(weatherPut({ [tag]: int(inside, width) }), { mutating: true }).ok).toBe(true);
        expect(decodeMetadataEnvelope(weatherPut({ [tag]: int(-inside, width) }), { mutating: true }).ok).toBe(true);
        for (const bad of [outside, -outside]) {
            expect(failure(decodeMetadataEnvelope(weatherPut({ [tag]: int(bad, width) }), { mutating: true }))).toEqual({
                category: "invalidDescriptor",
                detail: "invalidCombination",
            });
        }
    });

    it.each([
        [0, false],
        [1, true],
        [100_000, true],
        [100_001, false],
    ])("weather radius %i metres", (value, valid) => {
        const result = decodeMetadataEnvelope(weatherPut({ 0x8004: int(value, 4) }), { mutating: true });
        expect(result.ok).toBe(valid);
        if (!result.ok) expect(result.error.detail).toBe("invalidCombination");
    });

    it("bounds the same quantities in the weather request context response", () => {
        const body = new Uint8Array(96);
        const view = new DataView(body.buffer);
        view.setInt32(60, 2_000_000_000, true); // far past ±900,000,000 microdegrees
        view.setInt32(64, 0, true);
        view.setUint32(68, 50_000, true);
        body[88] = 1;
        const frame = controlRecord(OPCODE.QueryWeatherRequest, 0x01, 1, body);
        expect(failure(decodeControlFrame(frame))).toEqual({
            category: "invalidDescriptor",
            detail: "invalidCombination",
        });
    });
});

describe("encoders refuse rather than truncate (§1)", () => {
    it("refuses an Echo body that overflows the payload-length field", () => {
        const frame = {
            opcode: OPCODE.Echo,
            opcodeName: "Echo",
            requestId: requestId(1),
            response: false,
            error: false,
            more: false,
            body: { kind: "Echo", payload: new Uint8Array(70_000) },
        } as const;
        // 70,000 mod 65,536 is 4,464: exactly the silent wrap this refusal exists to prevent.
        expect(failure(encodeControlFrame(frame))).toEqual({ category: "invalidFrame", detail: "payloadLength" });
        expect(70_000 % 65_536).toBe(4_464);
    });

    it("refuses a payload above the 496-byte hard maximum", () => {
        const frame = {
            opcode: OPCODE.Echo,
            opcodeName: "Echo",
            requestId: requestId(1),
            response: false,
            error: false,
            more: false,
            body: { kind: "Echo", payload: new Uint8Array(MAX_CONTROL_PAYLOAD + 1) },
        } as const;
        expect(failure(encodeControlFrame(frame))).toEqual({ category: "invalidFrame", detail: "payloadLength" });
    });

    it("refuses an over-long diagnostic text", () => {
        const error = unwrap(decodeErrorBody(new Uint8Array(48).fill(0).map((_, i) => (i === 0 ? 5 : 0))));
        const frame = {
            opcode: OPCODE.StartUpload,
            opcodeName: "StartUpload",
            requestId: requestId(1),
            response: true,
            error: true,
            more: false,
            body: { kind: "Error", error: { ...error, text: new Uint8Array(300) } },
        } as const;
        // 300 mod 256 is 44 — the same class of wrap, one field narrower.
        expect(failure(encodeControlFrame(frame))).toEqual({ category: "invalidFrame", detail: "payloadLength" });
        expect(300 % 256).toBe(44);
    });

    it("refuses a frame above the negotiated maximum", () => {
        const frame = unwrap(decodeControlFrame(hexToBytes("4f4243500300010000000c00010000000303f4000004000000000000")));
        expect(encodeControlFrame(frame).ok).toBe(true);
        expect(failure(encodeControlFrame(frame, { maximumFrameBytes: 20 }))).toEqual({
            category: "invalidFrame",
            detail: "frameBounds",
        });
    });

    it("refuses a stream payload above the 4096-byte frame maximum", () => {
        const frame = {
            sessionId: 17,
            offset: 0n,
            direction: STREAM_DIRECTION.upload,
            flags: 0,
            payload: new Uint8Array(MAX_STREAM_FRAME_BYTES),
        };
        expect(failure(encodeStreamFrame(frame))).toEqual({ category: "invalidFrame", detail: "frameBounds" });
    });
});

describe("the registered envelope maxima (registry §4)", () => {
    /** The table in the registry is the authority; this recomputes it from the field specs. */
    const widest = (kind: ObjectKindName, role: SchemaRole): number | undefined => {
        const schema = metadataSchema(kind, role);
        if (schema === undefined) return undefined;
        let bytes = ENVELOPE_HEADER_BYTES;
        for (const field of schema.fields) {
            const width =
                field.type.kind === "text"
                    ? field.type.max
                    : field.type.kind === "bytes"
                      ? field.type.exact
                      : field.type.kind === "u8" || field.type.kind === "bool"
                        ? 1
                        : field.type.kind === "u16"
                          ? 2
                          : field.type.kind === "u32" || field.type.kind === "i32"
                            ? 4
                            : 8;
            bytes += 4 + width;
        }
        return bytes;
    };

    it.each([
        ["route", "put", 13],
        ["trip", "put", 8],
        ["ride", "put", 8],
        ["weather", "put", 68],
        ["volumeManifest", "put", 8],
        ["updatePackage", "put", 8],
        ["route", "patch", 70],
        ["volumeManifest", "patch", 13],
        ["route", "catalog", 82],
        ["trip", "catalog", 66],
        ["ride", "catalog", 41],
        ["weather", "catalog", 44],
        ["volumeManifest", "catalog", 55],
        ["updatePackage", "catalog", 77],
    ] as [ObjectKindName, SchemaRole, number][])("%s %s is %i bytes", (kind, role, expected) => {
        expect(metadataSchema(kind, role)?.maxBytes).toBe(expected);
        expect(widest(kind, role)).toBe(expected);
    });

    it("keeps every registered schema under its role's ceiling", () => {
        for (const kind of Object.keys(OBJECT_KIND) as ObjectKindName[]) {
            for (const role of ["put", "patch", "catalog"] as SchemaRole[]) {
                const schema = metadataSchema(kind, role);
                if (schema === undefined) continue;
                expect(schema.maxBytes).toBeLessThan(role === "catalog" ? CATALOG_METADATA_LIMIT : METADATA_ENVELOPE_LIMIT);
            }
        }
    });

    it("leaves SetMetadata unsupported on the kinds the registry says reject it", () => {
        for (const kind of ["trip", "ride", "weather", "updatePackage"] as ObjectKindName[]) {
            expect(metadataSchema(kind, "patch")).toBeUndefined();
        }
    });
});

describe("the identity model (system contract §identity)", () => {
    it("refuses a 128-bit identity of the wrong width", () => {
        expect(() => storeId(new Uint8Array(15))).toThrow();
        expect(() => operationId(new Uint8Array(17))).toThrow();
        expect(new Uint8Array(IDENTITY_BYTES).length).toBe(16);
    });

    it("refuses a zero SessionId or RequestId", () => {
        expect(() => sessionId(0)).toThrow();
        expect(() => requestId(0)).toThrow();
        expect(sessionId(1)).toBe(1);
    });

    it("copies identity bytes rather than aliasing the caller's buffer", () => {
        const source = new Uint8Array(16).fill(0xa1);
        const id = storeId(source);
        source[0] = 0x00;
        expect(id[0]).toBe(0xa1);
    });

    it("keeps the branded types from cross-assigning", () => {
        // The compile-time half of this is the assertion; the runtime half only proves the values
        // survive the brand. `@ts-expect-error` fails the build if the assignment ever type-checks.
        const store: StoreId = storeId(new Uint8Array(16));
        // @ts-expect-error a StoreId is not an OperationId
        const crossed: OperationId = store;
        expect(crossed.length).toBe(16);
    });
});

describe("u64 fields keep their full width (§1)", () => {
    it("decodes and re-encodes a value above 2^53 without loss", () => {
        const huge = (1n << 64n) - 1n;
        const body = new Uint8Array(20);
        const view = new DataView(body.buffer);
        view.setUint32(0, 7, true);
        view.setBigUint64(4, huge, true);
        view.setUint32(16, 1, true);
        const record = controlRecord(OPCODE.CheckpointUpload, 0x01, 3, body);
        const frame = unwrap(decodeControlFrame(record));
        if (frame.body.kind !== "CheckpointAccepted") throw new Error("expected a checkpoint response");
        expect(frame.body.response.durableNextOffset).toBe(huge);
        // Why `bigint` is not a preference: as doubles these two distinct offsets are one value, so
        // a `number` codec cannot represent the field width §1 calls normative.
        expect(Number(huge)).toBe(Number(huge - 1n));
        expect(frame.body.response.durableNextOffset).not.toBe(huge - 1n);
        expect(bytesToHex(unwrap(encodeControlFrame(frame)))).toBe(bytesToHex(record));
    });

    it("reports the advertised v3.0 bound without enforcing it in the codec", () => {
        expect(U64_V30_BOUND).toBe(0xffff_ffffn);
        expect(withinV30Bound(0xffff_ffffn)).toBe(true);
        expect(withinV30Bound(0x1_0000_0000n)).toBe(false);
        // The field width is normative even where the bound is not: a codec that truncated here
        // would be nonconforming, so decoding above the bound succeeds and the caller may check it.
        expect(logicalObjectId(0x1_0000_0000n)).toBe(0x1_0000_0000n);
    });
});

describe("decoding is total (§12)", () => {
    /** A cheap deterministic generator; the point is coverage of shapes, not cryptographic spread. */
    function* pseudoRandom(seed: number, count: number, maxLength: number): Generator<Uint8Array> {
        let state = seed >>> 0;
        const next = (): number => {
            state = (state * 1664525 + 1013904223) >>> 0;
            return state >>> 24;
        };
        for (let i = 0; i < count; i++) {
            const bytes = new Uint8Array(next() % maxLength);
            for (let b = 0; b < bytes.length; b++) bytes[b] = next();
            yield bytes;
        }
    }

    /** Every entry point the barrel exports. The module's promise is that none of them throws. */
    const ENTRY_POINTS: readonly [string, (bytes: Uint8Array) => DosResult<unknown>][] = [
        ["decodeControlFrame", (bytes) => decodeControlFrame(bytes)],
        ["decodeStreamFrame", (bytes) => decodeStreamFrame(bytes)],
        ["decodeErrorBody", (bytes) => decodeErrorBody(bytes)],
        ["decodeCapabilities", (bytes) => decodeCapabilities(bytes)],
        ["decodeSubjectEntry", (bytes) => decodeSubjectEntry(bytes)],
        ["decodeConfigBlock", (bytes) => decodeConfigBlock(bytes)],
        ["decodeMetadataEnvelope", (bytes) => decodeMetadataEnvelope(bytes, { mutating: true })],
    ];

    it.each(ENTRY_POINTS)("%s never throws on arbitrary bytes", (_name, decode) => {
        for (const bytes of pseudoRandom(1, 1500, 200)) {
            const result = decode(bytes);
            expect(typeof result.ok).toBe("boolean");
            if (!result.ok) expect(result.error.categoryValue).toBeGreaterThan(0);
        }
    });

    it("never throws on a valid frame truncated at every length", () => {
        const frame = hexToBytes("4f4243500300010000000c00010000000303f4000004000000000000");
        for (let length = 0; length <= frame.length; length++) {
            const result = decodeControlFrame(frame.subarray(0, length));
            expect(result.ok).toBe(length === frame.length);
        }
    });
});

describe("the §12 reserved details stay named", () => {
    it.each([
        ["busy", 5, "draftParts"],
        ["busy", 8, "maintenance"],
        ["busy", 10, "retainedPrevious"],
        ["insufficientSpace", 3, "retainedPrevious"],
        ["resourceLimit", 6, "draftParents"],
        ["resourceLimit", 12, "rideSlot"],
        ["catalogChanged", 3, "capabilitySnapshot"],
        ["objectNotFound", 2, "requestedRevision"],
        ["objectNotFound", 5, "resumableWork"],
    ] as const)("%s/%i is %s", (category, value, name) => {
        // These are registered but never emitted in v3.0. Their numbers stay burned, so a decoder
        // that reported them as "unknown" would lose the forward-diagnostic value of the row.
        expect(detailName(category, value)).toBe(name);
    });

    it("names detail zero as the no-narrower-fact value", () => {
        expect(detailName("busy", 0)).toBe("none");
        expect(detailName("busy", 250)).toBe("unknown");
    });
});

describe("the retained terminal replay signature (§11)", () => {
    it("is both claim bits set, guidance forced to reject-permanently, and no text", () => {
        // A durable record, not a live diagnosis: the presence requirements of §12 bind senders, and
        // a replay is exempt from them, so the decoder reads what the bits say and nothing more.
        const body = new Uint8Array(48);
        const view = new DataView(body.buffer);
        view.setUint16(0, CATEGORY.busy, true);
        body[6] = GUIDANCE.rejectPermanently;
        view.setUint16(8, PRESENCE.durableClaimExists | PRESENCE.claimIsTerminal, true);
        const decoded = unwrap(decodeErrorBody(body));
        expect(isRetainedTerminalReplay(decoded)).toBe(true);
        expect(decoded.guidance).toBe(GUIDANCE.rejectPermanently);
        expect(decoded.owner).toBe(0);
        expect(errorText(decoded)).toBe("");
    });
});

describe("the stream fault body (§13)", () => {
    /** A nonterminal fault status carrying one category/detail pair. */
    const faultFrame = (category: number, detail: number): Uint8Array => {
        const bytes = new Uint8Array(16 + 24);
        const view = new DataView(bytes.buffer);
        view.setUint32(0, 17, true);
        view.setUint16(12, 24, true);
        bytes[14] = STREAM_DIRECTION.status;
        bytes[15] = STREAM_FLAG.fault;
        view.setUint16(16, category, true);
        view.setUint16(18, detail, true);
        view.setBigUint64(20, 4n, true);
        view.setBigUint64(28, 4n, true);
        return bytes;
    };

    const TRANSPORT = [
        "invalidFrame",
        "invalidDescriptor",
        "invalidOffset",
        "invalidSession",
        "checksumFailure",
        "mediaUnavailable",
        "mediaIo",
        "cancelled",
        "linkLost",
        "internal",
    ] as const;

    it("is exactly ten categories", () => {
        expect(TRANSPORT.length).toBe(10);
    });

    it.each(TRANSPORT)("accepts %s", (category) => {
        const decoded = decodeStreamFrame(faultFrame(CATEGORY[category], 1));
        expect(decoded.ok).toBe(true);
        if (decoded.ok) expect(decoded.value.fault?.category).toBe(category);
    });

    it.each((Object.keys(CATEGORY) as (keyof typeof CATEGORY)[]).filter((c) => !TRANSPORT.includes(c as never)))(
        "rejects %s as unknownEnum",
        (category) => {
            // §13 froze the set as closed, and `resourceLimit` is the boundary member: every bounded
            // resource a stream could exhaust is reserved at admission, so an attached session has no
            // resource-limit condition to report. It rejects exactly like the domain categories the
            // checked-in negatives already pin.
            expect(failure(decodeStreamFrame(faultFrame(CATEGORY[category], 0)))).toEqual({
                category: "invalidDescriptor",
                detail: "unknownEnum",
            });
        },
    );

    it("preserves an unregistered detail instead of rejecting it", () => {
        // §12: "Unknown received details are preserved for forward diagnostics but do not change
        // category retry behavior." The category is closed; the detail is not.
        const decoded = unwrap(decodeStreamFrame(faultFrame(CATEGORY.invalidOffset, 250)));
        expect(decoded.fault?.detailValue).toBe(250);
        expect(decoded.fault?.detail).toBe("unknown");
        expect(decoded.fault?.category).toBe("invalidOffset");
    });
});

describe("frame-limit derivation beyond the fixture's own cases (§14.0)", () => {
    it("takes the smaller of the two advertised maxima", () => {
        expect(negotiateFrameLimit("control", 512, 244, 512)).toEqual({ outcome: "negotiated", negotiated: 244 });
        expect(negotiateFrameLimit("control", 512, 512, 256)).toEqual({ outcome: "negotiated", negotiated: 256 });
    });

    it("fails closed at each floor", () => {
        expect(negotiateFrameLimit("control", MIN_CONTROL_FRAME - 1, 512, 512).outcome).toBe("belowProtocolMinimum");
        expect(negotiateFrameLimit("control", MIN_STREAM_FRAME - 1, 512, 512).outcome).toBe("undeliverable");
        expect(negotiateFrameLimit("stream", MIN_STREAM_FRAME - 1, 4096, 4096).outcome).toBe("belowProtocolMinimum");
        expect(negotiateFrameLimit("stream", MIN_STREAM_FRAME, 4096, 4096)).toEqual({
            outcome: "negotiated",
            negotiated: MIN_STREAM_FRAME,
        });
    });

    it("rejects an advertised maximum above the hard bound but decodes one below the floor", () => {
        const hello = (control: number, stream: number): Uint8Array => {
            const body = new Uint8Array(12);
            const view = new DataView(body.buffer);
            body[0] = 3;
            body[1] = 3;
            view.setUint16(2, control, true);
            view.setUint16(4, stream, true);
            return controlRecord(OPCODE.Hello, 0, 1, body);
        };
        expect(failure(decodeControlFrame(hello(600, 1024)))).toEqual({
            category: "invalidFrame",
            detail: "frameBounds",
        });
        expect(failure(decodeControlFrame(hello(244, 8192)))).toEqual({
            category: "invalidFrame",
            detail: "frameBounds",
        });
        // Below the floor is a real condition the device answers with resourceLimit, so it decodes.
        expect(decodeControlFrame(hello(191, 1024)).ok).toBe(true);
        expect(negotiateFrameLimit("control", 191, 191, 512).outcome).toBe("belowProtocolMinimum");
    });
});
