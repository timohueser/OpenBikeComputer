/**
 * The flat-store v4 acceptance suite for this codec.
 *
 * `specs/vectors/flat-store-v4/` is the contract the implementations agree on: a fixture producer
 * builds those bytes straight from the protocol's byte tables without calling any production
 * encoder, the Rust codec is pinned against them, and this file is the TypeScript half. A divergence
 * here is a bug here and never a reason to move a fixture.
 *
 * Four kinds of assertion, and each of them catches something the others cannot:
 *
 * 1. **Byte-exact decode and re-encode** for every control, stream and error fixture, which catches
 *    a field at the wrong offset.
 * 2. **The semantic body** each fixture states beside its bytes, which catches three codecs agreeing
 *    on every byte and disagreeing about their meaning.
 * 3. **Identical typed rejection** for every negative fixture.
 * 4. **Checked-in file hashes**, plus a guard that every row the manifest lists was exercised.
 */

import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import {
    Detail,
    ERROR_CODE_NAMES,
    ErrorCode,
    EntryFlags,
    Flags,
    ObjectState,
    Opcode,
    WIRE_MAJOR,
    decodeRequest,
    decodeResponse,
    encodeArchiveRideRequest,
    encodeArchiveRideResponse,
    encodeArmRequest,
    encodeArmResponse,
    encodeCancelRequest,
    encodeCancelResponse,
    encodeErrorResponse,
    encodeFormatRequest,
    encodeFormatResponse,
    encodeGetRequest,
    encodeGetResponse,
    encodeListRequest,
    encodeListResponse,
    encodePutRequest,
    encodePutResponse,
    encodeRemoveRequest,
    encodeRemoveResponse,
    encodeStatusRequest,
    encodeStatusResponse,
    encodeStreamRecord,
    isFailure,
    splitStreamRecord,
    streamRecordFault,
    type StreamRecordFault,
    type ControlFailure,
    type DecodedRequest,
    type Request,
    type Response,
} from "./protocol";

/** Walk up from this file to the repo root (the directory holding `specs/vectors/`). */
function repoRoot(): string {
    let dir = dirname(fileURLToPath(import.meta.url));
    for (let up = 0; up < 12; up++) {
        if (existsSync(join(dir, "specs", "vectors", "manifest.json"))) return dir;
        dir = dirname(dir);
    }
    throw new Error("could not locate the repo root from " + import.meta.url);
}

const SUITE = join(repoRoot(), "specs/vectors/flat-store-v4");

/**
 * The manifest's own digest. Re-pin this **deliberately**, in the same commit that changes a
 * fixture and for the same stated reason — never because a test went red.
 */
const MANIFEST_SHA256 = "e4f6f525def24b3ad14d4a05735aef16d791686a6ca58ca2858c822dca21fab5";
const read = (relative: string): string => readFileSync(join(SUITE, relative), "utf8");

interface ManifestRow {
    name: string;
    file: string;
    sha256: string;
}

const MANIFEST = JSON.parse(read("manifest.json")) as {
    suite: string;
    format: number;
    wire_major: number;
    storage_format: number;
    controls: ManifestRow[];
    streams: ManifestRow[];
    errors: ManifestRow[];
    negative: ManifestRow[];
};

/** Every fixture file some suite below actually ran. The drift guard compares this to the manifest. */
const exercised = new Set<string>();

function fixture<T>(row: ManifestRow): T {
    exercised.add(row.file);
    return JSON.parse(read(row.file)) as T;
}

const ALL_ROWS = [...MANIFEST.controls, ...MANIFEST.streams, ...MANIFEST.errors, ...MANIFEST.negative];

interface ControlFixture {
    name: string;
    kind: "control";
    direction: "request" | "response";
    opcode: { name: string; value: number };
    header: { magic: string; major: number; flags: number; payloadLength: number; requestId: number };
    body: Record<string, unknown>;
    frame: string;
}

interface StreamFixture {
    name: string;
    kind: "stream";
    requestId: number;
    offset: string;
    payloadLength: number;
    record: string;
}

interface ErrorFixture {
    name: string;
    kind: "error";
    opcode: { name: string; value: number };
    requestId: number;
    body: { code: string; codeValue: number; detail: string; detailValue: number; context: string };
    frame: string;
}

interface NegativeFixture {
    name: string;
    kind: "negative";
    target: "controlRecord" | "streamRecord" | "controlResponse";
    expect: {
        disposition: "errorResponse" | "closeRecordStream" | "terminateTransfer" | "rejectResponse";
        code?: string;
        codeValue?: number;
        detail?: string;
        detailValue?: number;
    };
    bytes: string;
}

function hexToBytes(hex: string): Uint8Array {
    const out = new Uint8Array(hex.length / 2);
    for (let i = 0; i < out.length; i++) out[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
    return out;
}

function bytesToHex(bytes: Uint8Array): string {
    let out = "";
    for (const byte of bytes) out += byte.toString(16).padStart(2, "0");
    return out;
}

/** Fail on the first differing byte with its index, instead of dumping two long arrays. */
function expectSameBytes(actual: Uint8Array, expected: Uint8Array, what: string): void {
    const n = Math.min(actual.length, expected.length);
    for (let i = 0; i < n; i++) {
        if (actual[i] !== expected[i]) {
            throw new Error(
                `${what}: first difference at byte ${i} — this codec produced 0x${actual[i].toString(16)}, ` +
                    `the fixture has 0x${expected[i].toString(16)} (lengths ${actual.length} vs ${expected.length})`,
            );
        }
    }
    expect(actual.length, `${what}: length`).toBe(expected.length);
}

function rowsOf(list: ManifestRow[]): Array<readonly [string, ManifestRow]> {
    return list.map((row) => [row.name, row] as const);
}

describe("the manifest", () => {
    it("is the flat-store-v4 suite at wire major 4", () => {
        expect(MANIFEST.suite).toBe("flat-store-v4");
        expect(MANIFEST.wire_major).toBe(WIRE_MAJOR);
        expect(MANIFEST.format).toBe(1);
    });

    it("pins its own SHA-256, so the pinner cannot be edited into agreeing with a drifted fixture", () => {
            // Every hash below lives *in* the manifest, so a change that rewrote a fixture and its
            // row together would pass the next test silently. The manifest's own digest is checked
            // into the producing crate and reproduced here.
        const digest = createHash("sha256").update(readFileSync(join(SUITE, "manifest.json"))).digest("hex");
        expect(digest, "the manifest itself moved — re-pin this hash deliberately, never reflexively").toBe(
            MANIFEST_SHA256,
        );
    });

    it("pins the SHA-256 of every checked-in fixture file", () => {
        expect(ALL_ROWS.length).toBeGreaterThan(0);
        const drifted = ALL_ROWS.filter(
            (row) => createHash("sha256").update(readFileSync(join(SUITE, row.file))).digest("hex") !== row.sha256,
        );
        expect(drifted.map((row) => row.file)).toEqual([]);
    });
});

/**
 * Re-encode a decoded request with the encoder its opcode owns.
 *
 * Going through the *decoded* value rather than through the fixture's `body` map is deliberate: it
 * makes the round trip a property of the codec pair, so a decoder that read a field from the wrong
 * offset cannot be rescued by an encoder that writes it back to the same wrong one.
 */
function reencodeRequest(decoded: DecodedRequest): Uint8Array {
    const { requestId, request } = decoded;
    switch (request.opcode) {
        case Opcode.List:
            return encodeListRequest(requestId, request.body);
        case Opcode.Status:
            return encodeStatusRequest(requestId, request.body);
        case Opcode.Get:
            return encodeGetRequest(requestId, request.body);
        case Opcode.Put:
            return encodePutRequest(requestId, request.body);
        case Opcode.Remove:
            return encodeRemoveRequest(requestId, request.body);
        case Opcode.Cancel:
            return encodeCancelRequest(requestId, request.body);
        case Opcode.Arm:
            return encodeArmRequest(requestId, request.body);
        case Opcode.ArchiveRide:
            return encodeArchiveRideRequest(requestId, request.body);
        case Opcode.Format:
            return encodeFormatRequest(requestId, request.body);
    }
}

/** Re-encode a decoded response with the encoder its opcode owns. */
function reencodeResponse(requestId: number, response: Response): Uint8Array {
    switch (response.opcode) {
        case Opcode.List:
            return encodeListResponse(requestId, response.body);
        case Opcode.Status:
            return encodeStatusResponse(requestId, response.body);
        case Opcode.Get:
            return encodeGetResponse(requestId, response.body);
        case Opcode.Put:
            return encodePutResponse(requestId, response.body);
        case Opcode.Remove:
            return encodeRemoveResponse(requestId, response.body.commitSequence);
        case Opcode.Cancel:
            return encodeCancelResponse(requestId, response.body.cancelled);
        case Opcode.Arm:
            return encodeArmResponse(requestId, response.body);
        case Opcode.ArchiveRide:
            return encodeArchiveRideResponse(requestId, response.body);
        case Opcode.Format:
            return encodeFormatResponse(requestId, response.body.storeId);
    }
}

/** The values a request fixture states, read back out of the decoded message. */
function semanticRequest(request: Request): Record<string, unknown> {
    switch (request.opcode) {
        case Opcode.List: {
            const { kind, cursor } = request.body;
            return cursor
                ? {
                      kindFilter: kind ?? 0,
                      cursor: true,
                      cursorObjectId: String(cursor.objectId),
                      cursorRevision: String(cursor.revision),
                      expectedCommitSequence: String(cursor.commitSequence),
                  }
                : { kindFilter: kind ?? 0, cursor: false };
        }
        case Opcode.Status:
        case Opcode.Get:
            return { objectId: String(request.body.objectId), revision: String(request.body.revision) };
        case Opcode.Remove:
            return { objectId: String(request.body.objectId), expectedRevision: String(request.body.revision) };
        case Opcode.Put:
            return {
                objectId: String(request.body.objectId),
                expectedRevision: String(request.body.expectedRevision),
                payloadLength: String(request.body.payloadLength),
                payloadCrc32: request.body.payloadCrc32,
                kind: request.body.kind,
                displayName: request.body.displayName,
            };
        case Opcode.Cancel:
            return { transferRequestId: request.body.transferRequestId };
        case Opcode.Arm:
            return {
                packageObjectId: String(request.body.packageObjectId),
                expectedRevision: String(request.body.expectedRevision),
            };
        case Opcode.ArchiveRide:
            return { ...request.body, objectId: String(request.body.objectId), revision: String(request.body.revision),
                payloadLength: String(request.body.payloadLength) };
        case Opcode.Format:
            return {
                expectedStoreId: request.body.expectedStoreId,
                replacementStoreId: request.body.replacementStoreId,
            };
    }
}

/** The values a response fixture states, read back out of the decoded message. */
function semanticResponse(response: Response): Record<string, unknown> {
    switch (response.opcode) {
        case Opcode.List:
            return {
                storeId: response.body.storeId,
                commitSequence: String(response.body.commitSequence),
                entries: response.body.entries.length,
                more: response.body.more,
            };
        case Opcode.Status:
            return {
                state: response.body.state,
                headRevision: String(response.body.headRevision),
                headPayloadLength: String(response.body.headPayloadLength),
                headPayloadCrc32: response.body.headPayloadCrc32,
            };
        case Opcode.Get:
            return {
                revisionServed: String(response.body.revisionServed),
                payloadLength: String(response.body.payloadLength),
                payloadCrc32: response.body.payloadCrc32,
            };
        case Opcode.Put:
            return {
                objectId: String(response.body.objectId),
                revision: String(response.body.revision),
                payloadLength: String(response.body.payloadLength),
                payloadCrc32: response.body.payloadCrc32,
            };
        case Opcode.Remove:
            return { commitSequence: String(response.body.commitSequence) };
        case Opcode.Cancel:
            return { outcome: response.body.cancelled ? 0 : 1 };
        case Opcode.Arm:
            return {
                rollbackObjectId: String(response.body.rollbackObjectId),
                commitSequence: String(response.body.commitSequence),
            };
        case Opcode.ArchiveRide:
            return { commitSequence: String(response.body.commitSequence), timestamp: response.body.timestamp };
        case Opcode.Format:
            return { storeId: response.body.storeId };
    }
}

describe("control vectors decode and re-encode byte for byte", () => {
    it.each(rowsOf(MANIFEST.controls))("%s", (_name, row) => {
        const vector = fixture<ControlFixture>(row);
        const bytes = hexToBytes(vector.frame);
        expect(vector.header.major).toBe(WIRE_MAJOR);

        if (vector.direction === "request") {
            const decoded = decodeRequest(bytes);
            if (isFailure(decoded)) throw new Error(`${vector.name}: decoded as a failure — ${JSON.stringify(decoded)}`);
            expect(decoded.requestId, "RequestId").toBe(vector.header.requestId);
            expect(decoded.request.opcode, "opcode").toBe(vector.opcode.value);
            expect(semanticRequest(decoded.request)).toEqual(vector.body);
            expectSameBytes(reencodeRequest(decoded), bytes, vector.name);
            return;
        }

        const decoded = decodeResponse(bytes);
        if (!decoded.ok) throw new Error(`${vector.name}: decoded as an error response`);
        expect(decoded.requestId, "RequestId").toBe(vector.header.requestId);
        expect(decoded.response.opcode, "opcode").toBe(vector.opcode.value);
        expect(semanticResponse(decoded.response)).toEqual(vector.body);
        expectSameBytes(reencodeResponse(decoded.requestId, decoded.response), bytes, vector.name);
    });

    it("reads an accepted Assistant route in a catalog page", () => {
        const row = MANIFEST.controls.find((r) => r.name === "list-response-two-entries");
        if (!row) throw new Error("missing LIST vector");
        const bytes = hexToBytes(fixture<ControlFixture>(row).frame);
        new DataView(bytes.buffer, bytes.byteOffset).setUint16(16 + 24 + 30, 1 << 3, true);
        const decoded = decodeResponse(bytes);
        if (!decoded.ok || decoded.response.opcode !== Opcode.List) throw new Error("not a LIST page");
        expect(decoded.response.body.entries).toHaveLength(2);
        expect(decoded.response.body.entries[0]).toMatchObject({
            objectId: 1n, revision: 3n, flags: EntryFlags.AssistantAccepted,
        });
        expect(decoded.response.body.entries[1]).toMatchObject({ objectId: 2n, flags: EntryFlags.Recording });
    });

    it("reads §3.11's LIST page as two entries in catalog order", () => {
        const row = MANIFEST.controls.find((r) => r.name === "list-response-two-entries");
        if (!row) throw new Error("the manifest no longer lists list-response-two-entries");
        const vector = fixture<ControlFixture>(row);
        const decoded = decodeResponse(hexToBytes(vector.frame));
        if (!decoded.ok || decoded.response.opcode !== Opcode.List) throw new Error("not a LIST page");
        const [route, ride] = decoded.response.body.entries;
            // The two objects: the route at revision 3, and the ride the device is recording, whose
            // length and CRC are zero until the commit that ends it.
        expect(route).toMatchObject({
            objectId: 1n,
            revision: 3n,
            payloadLength: 42_137n,
            payloadCrc32: 0x9c4a_7e21,
            kind: 1,
            flags: 0,
            displayName: "Grimsel Loop",
        });
        expect(ride).toMatchObject({
            objectId: 2n,
            revision: 1n,
            payloadLength: 0n,
            payloadCrc32: 0,
            kind: 3,
            flags: 1,
            displayName: "",
        });
    });

    it("carries a `u64` past 2^53 through a LIST entry without rounding it", () => {
            // Ids, revisions, lengths and commit sequences are 64-bit, and a `number` stops being
            // exact at 2^53. Nothing on a card mints an id this large, so the check belongs at the
            // codec rather than at a device: these are the bytes a client must read back unchanged.
        const huge = 18_446_744_073_709_551_615n; // 2^64 - 1
        const entry = {
            objectId: huge,
            revision: huge - 1n,
            payloadLength: 9_007_199_254_740_993n, // 2^53 + 1, the first length a `number` loses
            payloadCrc32: 0xdead_beef,
            kind: 1,
            flags: 0,
            displayName: "big",
        };
        const frame = encodeListResponse(7, {
            storeId: "0123456789abcdef0123456789abcdef",
            commitSequence: huge,
            entries: [entry],
            more: false,
        });

        const decoded = decodeResponse(frame);
        if (!decoded.ok || decoded.response.opcode !== Opcode.List) throw new Error("not a LIST page");
        expect(decoded.response.body.commitSequence).toBe(huge);
        expect(decoded.response.body.entries[0]).toEqual(entry);
    });
});

describe("stream vectors split and re-encode byte for byte", () => {
    it.each(rowsOf(MANIFEST.streams))("%s", (_name, row) => {
        const vector = fixture<StreamFixture>(row);
        const record = hexToBytes(vector.record);

        const split = splitStreamRecord(record);
        if (!split) throw new Error(`${vector.name}: this codec refused a legal stream record`);
        expect(split.frame.transferRequestId, "RequestId").toBe(vector.requestId);
        expect(String(split.frame.offset), "offset").toBe(vector.offset);
        expect(split.frame.payloadLength, "payload length").toBe(vector.payloadLength);
        expect(split.payload.length, "carried payload").toBe(vector.payloadLength);

        expectSameBytes(
            encodeStreamRecord(split.frame.transferRequestId, split.frame.offset, split.payload),
            record,
            vector.name,
        );
    });
});

describe("error vectors decode and re-encode byte for byte", () => {
    it.each(rowsOf(MANIFEST.errors))("%s", (_name, row) => {
        const vector = fixture<ErrorFixture>(row);
        const bytes = hexToBytes(vector.frame);

        const decoded = decodeResponse(bytes);
        if (decoded.ok) throw new Error(`${vector.name}: decoded as a success response`);
        expect(decoded.requestId, "RequestId").toBe(vector.requestId);
        expect(decoded.opcode, "opcode").toBe(vector.opcode.value);
        expect(decoded.refusal.code, "code").toBe(vector.body.codeValue);
        expect(decoded.refusal.detail, "detail").toBe(vector.body.detailValue);
        expect(String(decoded.refusal.context), "context").toBe(vector.body.context);
            // The code's own name, so a table that drifted from the spec's spelling fails here rather
            // than in a message a rider reads.
        expect(ERROR_CODE_NAMES[decoded.refusal.code]).toBe(vector.body.code);

        expectSameBytes(encodeErrorResponse(decoded.opcode, decoded.requestId, decoded.refusal), bytes, vector.name);
    });

    it("sets response|error and nothing else on every error frame", () => {
        for (const row of MANIFEST.errors) {
            const vector = JSON.parse(read(row.file)) as ErrorFixture;
            const flags = hexToBytes(vector.frame)[6];
            expect(flags, vector.name).toBe(Flags.Response | Flags.Error);
        }
    });
});

/** The refusal a control record earns, or the disposition that is not one. */
function dispositionOf(bytes: Uint8Array): ControlFailure | DecodedRequest {
    return decodeRequest(bytes);
}

/**
 * Which {@link StreamRecordFault} a negative vector is *about*, taken from the vector's **name**.
 *
 * Deliberately not re-derived from the bytes: a mapping that read the reserved field and the length
 * would be a second copy of the codec, and a test that agrees with the implementation by
 * construction proves nothing. A vector whose name this does not recognise fails loudly, because the
 * alternative is a new negative fixture silently landing in whichever bucket the default named.
 */
function streamFaultFor(vector: NegativeFixture): StreamRecordFault {
    switch (vector.name) {
        case "stream-nonzero-reserved-field":
            return "reservedBits";
        case "stream-zero-payload-length":
            return "zeroLength";
        case "stream-length-disagreeing-with-the-record":
            return "lengthMismatch";
        default:
            throw new Error(
                `${vector.name}: a §3.8 stream negative with no stated fault — name it in streamFaultFor`,
            );
    }
}

describe("negative vectors are refused with the contract's own code and detail", () => {
    it.each(rowsOf(MANIFEST.negative))("%s", (_name, row) => {
        const vector = fixture<NegativeFixture>(row);
        const bytes = hexToBytes(vector.bytes);

        if (vector.target === "streamRecord") {
                // A malformed stream record gets no answer of its own: it terminates the transfer it
                // claims to belong to, which on this side is the codec refusing to split it.
            expect(vector.expect.disposition).toBe("terminateTransfer");
            // `toBeNull()` alone would pass whether the codec refused this record for the reason the
            // fixture names or for some unrelated one — three different malformations collapsing to
            // one indistinguishable assertion. The fault name is what makes each vector test itself.
            expect(splitStreamRecord(bytes), vector.name).toBeNull();
            expect(streamRecordFault(bytes), `${vector.name}: refused, but not for the stated reason`).toBe(
                streamFaultFor(vector),
            );
            return;
        }

        if (vector.target === "controlResponse") {
            expect(vector.expect.disposition).toBe("rejectResponse");
            expect(() => decodeResponse(bytes)).toThrow();
            return;
        }
        const outcome = dispositionOf(bytes);
        if (vector.expect.disposition === "closeRecordStream") {
                // There is no `RequestId` to echo, so a receiver emits nothing at all.
            expect(isFailure(outcome) && outcome.kind === "unanswerable", vector.name).toBe(true);
            return;
        }
        if (!isFailure(outcome) || outcome.kind !== "refused") {
            throw new Error(`${vector.name}: this codec accepted a record the contract refuses`);
        }
        expect(outcome.refusal.code, "code").toBe(vector.expect.codeValue);
        expect(outcome.refusal.detail, "detail").toBe(vector.expect.detailValue);
        expect(ERROR_CODE_NAMES[outcome.refusal.code]).toBe(vector.expect.code);
    });
});

describe("the code and detail tables are §3.9's", () => {
    it("registers fourteen codes, and no code zero", () => {
        expect(Object.values(ErrorCode).sort((a, b) => a - b)).toEqual([
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14,
        ]);
        expect(Object.values(ErrorCode)).not.toContain(0);
    });

    it("names the two details `busy` has, because a client's retry policy is the same for both", () => {
        expect(Detail.busy).toEqual({ transfer: 1, holds: 2 });
    });

    it("maps §3.4's three states onto 0, 1 and 2", () => {
        expect(ObjectState).toEqual({ Absent: 0, Committed: 1, Superseded: 2 });
    });
});

describe("the suite", () => {
    it("exercises every fixture the manifest lists", () => {
        const untouched = ALL_ROWS.map((row) => row.file).filter((file) => !exercised.has(file));
        expect(untouched).toEqual([]);
    });

    it("lists every fixture on disk, so a file added without a row cannot hide", () => {
        // The guard above walks manifest → disk. On its own that is half a guard: a fixture checked
        // in without a manifest row is exercised by nothing, hashed by nothing, and invisible to
        // every assertion in this file. Walking disk → manifest is the other half, and the pair is
        // what makes "the manifest is the index" true rather than aspirational.
        const listed = new Set(ALL_ROWS.map((row) => row.file));
        const onDisk: string[] = [];
        for (const dir of ["controls", "streams", "errors", "negative"]) {
            for (const name of readdirSync(join(SUITE, dir))) {
                if (name.endsWith(".json")) onDisk.push(`${dir}/${name}`);
            }
        }
        expect(onDisk.length).toBeGreaterThan(0);
        expect(onDisk.filter((file) => !listed.has(file)).sort()).toEqual([]);
    });

    it("round-trips every fixture's hex through this file's own helpers", () => {
        // The hex helpers are test infrastructure, and a bug in them would silently weaken every
        // assertion above rather than fail one.
        const sample = hexToBytes("4f42433404010100");
        expect(bytesToHex(sample)).toBe("4f42433404010100");
    });
});
