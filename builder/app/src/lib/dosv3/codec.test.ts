/**
 * The properties the shared vectors deliberately do *not* carry as bytes.
 *
 * `Device_Object_Vectors_v2.md` §2.2 is explicit about one of them: the schema ceilings are
 * asserted arithmetically rather than as fixtures, "because no legal envelope reaches one, so a
 * ceiling fixture would necessarily be a fixture a conforming decoder must reject." The rest are
 * the same shape — the identity model, the 64-bit field width, and totality — facts about the codec
 * that a byte vector cannot state.
 */

import { describe, expect, it } from "vitest";

import { Writer, bytesToHex, hexToBytes } from "./bytes";
import {
    CATALOG_METADATA_LIMIT,
    METADATA_ENVELOPE_LIMIT,
    MIN_CONTROL_FRAME,
    MIN_STREAM_FRAME,
    negotiateFrameLimit,
} from "./capabilities";
import { PRESENCE, decodeErrorBody, errorText, isRetainedTerminalReplay } from "./errorBody";
import {
    CATALOG_ENTRY_PREFIX_BYTES,
    CATALOG_PAGE_PREFIX_BYTES,
    decodeCheckpointResponse,
    encodeCheckpointResponse,
} from "./messages";
import { decodeControlFrame } from "./frame";
import {
    IDENTITY_BYTES,
    U64_V30_BOUND,
    logicalObjectId,
    operationId,
    requestId,
    sessionId,
    storeId,
    withinV30Bound,
    type OperationId,
    type StoreId,
} from "./ids";
import { ENVELOPE_HEADER_BYTES, OBJECT_KIND, metadataSchema, type ObjectKindName, type SchemaRole } from "./registry";
import { CATEGORY, GUIDANCE, decoding, detailName, type CategoryName } from "./result";
import { FAULT_BODY_BYTES, STREAM_DIRECTION, STREAM_FLAG, decodeStreamFrame } from "./stream";

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
        const routeCatalog = metadataSchema("route", "catalog");
        const weatherPut = metadataSchema("weather", "put");
        expect(CATALOG_PAGE_PREFIX_BYTES + CATALOG_ENTRY_PREFIX_BYTES + (routeCatalog?.maxBytes ?? 0)).toBe(162);
        expect(48 + (weatherPut?.maxBytes ?? 0)).toBe(116);
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
        const bytes = new Writer(20).u32(7).u64(huge).u32(0).u32(1).finish();
        const decoded = decodeCheckpointResponse(bytes);
        expect(decoded.durableNextOffset).toBe(huge);
        // Why `bigint` is not a preference: as doubles these two distinct offsets are one value, so
        // a `number` codec cannot represent the field width §1 calls normative.
        expect(Number(huge)).toBe(Number(huge - 1n));
        expect(decoded.durableNextOffset).not.toBe(huge - 1n);
        expect(bytesToHex(encodeCheckpointResponse(decoded))).toBe(bytesToHex(bytes));
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

    it("never throws on arbitrary control bytes", () => {
        for (const bytes of pseudoRandom(1, 2000, 200)) {
            const result = decodeControlFrame(bytes);
            expect(typeof result.ok).toBe("boolean");
            if (!result.ok) expect(result.error.categoryValue).toBeGreaterThan(0);
        }
    });

    it("never throws on arbitrary stream bytes", () => {
        for (const bytes of pseudoRandom(2, 2000, 96)) {
            const result = decoding(() => decodeStreamFrame(bytes));
            expect(typeof result.ok).toBe("boolean");
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
        const body = new Writer(48)
            .u16(5) // busy
            .u16(0)
            .u16(0)
            .u8(GUIDANCE.rejectPermanently)
            .u8(0)
            .u16(PRESENCE.durableClaimExists | PRESENCE.claimIsTerminal)
            .u32(0)
            .u64(0n)
            .u64(0n)
            .u64(0n)
            .u64(0n)
            .u8(0)
            .zeros(1)
            .finish();
        const decoded = decodeErrorBody(body);
        expect(isRetainedTerminalReplay(decoded)).toBe(true);
        expect(decoded.guidance).toBe(GUIDANCE.rejectPermanently);
        expect(decoded.owner).toBe(0);
        expect(errorText(decoded)).toBe("");
    });
});

describe("the stream fault-body transport set (§13)", () => {
    /** A nonterminal fault status carrying one category/detail pair. */
    const faultFrame = (category: number, detail: number): Uint8Array =>
        new Writer(16 + FAULT_BODY_BYTES)
            .u32(17)
            .u64(0n)
            .u16(FAULT_BODY_BYTES)
            .u8(STREAM_DIRECTION.status)
            .u8(STREAM_FLAG.fault)
            .u16(category)
            .u16(detail)
            .u64(4n)
            .u64(4n)
            .u8(0)
            .zeros(3)
            .finish();

    const TRANSPORT: readonly CategoryName[] = [
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
    ];

    it("is exactly ten categories", () => {
        expect(TRANSPORT.length).toBe(10);
    });

    it.each(TRANSPORT)("accepts %s", (category) => {
        const decoded = decoding(() => decodeStreamFrame(faultFrame(CATEGORY[category], 1)));
        expect(decoded.ok).toBe(true);
        if (decoded.ok) expect(decoded.value.fault?.category).toBe(category);
    });

    it.each(
        (Object.keys(CATEGORY) as CategoryName[]).filter((category) => !TRANSPORT.includes(category)),
    )("rejects %s as unknownEnum", (category) => {
        // §13 froze the set as closed, and `resourceLimit` is the boundary member: every bounded
        // resource a stream could exhaust is reserved at admission, so an attached session has no
        // resource-limit condition to report. It rejects exactly like the domain categories the
        // checked-in negatives already pin.
        const decoded = decoding(() => decodeStreamFrame(faultFrame(CATEGORY[category], 0)));
        expect(decoded.ok).toBe(false);
        if (!decoded.ok) {
            expect(decoded.error.category).toBe("invalidDescriptor");
            expect(decoded.error.detail).toBe("unknownEnum");
            expect(decoded.error.detailValue).toBe(2);
        }
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
});
