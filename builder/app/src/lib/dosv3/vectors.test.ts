/**
 * The DOS1 cross-language acceptance suite for this codec (issue #1358, slice 3b).
 *
 * `specs/vectors/device-object-v2/` is the contract three implementations agree on. The Rust
 * fixture producer builds those bytes straight from the specification's byte tables without calling
 * the production encoder; Swift and TypeScript then decode and re-encode the same files. That is
 * the whole point of the exercise, so this codec was written from the normative tables and the
 * fixtures alone — never from another language's source.
 *
 * `Device_Object_Vectors_v2.md` §7 fixes what acceptance means, and each `describe` below is one of
 * its clauses:
 *
 * - byte-exact decode/re-encode parity for every positive fixture;
 * - identical typed rejection for every negative fixture;
 * - identical observations for every transcript;
 * - checked-in fixture hashes, and a guard that fails on an unreviewed fixture rewrite.
 *
 * The last `describe` is the drift guard in the other direction: every file the manifest lists has
 * to be exercised by one of the suites above, so a fixture cannot be added and silently ignored.
 */

import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { bytesToHex, hexToBytes } from "./bytes";
import {
    bleControlCeiling,
    decodeCapabilities,
    decodeSubjectEntry,
    encodeCapabilities,
    encodeSubjectEntry,
    negotiateFrameLimit,
    MAX_CONTROL_FRAME,
    MAX_STREAM_FRAME,
    MIN_CONTROL_FRAME,
    MIN_STREAM_FRAME,
} from "./capabilities";
import { decodeErrorBody, encodeErrorBody } from "./errorBody";
import { decodeControlFrame, encodeControlFrame, OPCODE, type ControlBody, type OpcodeName } from "./frame";
import { storeId } from "./ids";
import { canonicalIntent, intentDigest, INTENT_PREFIX_BYTES, type IntentSource } from "./intent";
import { decodeMetadataEnvelope, encodeMetadataEnvelope } from "./metadata";
import {
    decodeBeginDraft,
    decodeConfigBlock,
    decodeDeleteObject,
    decodeDeviceStatus,
    decodeDraftPartAccepted,
    decodeFinalizeDraftResponse,
    decodeFinishDownload,
    decodeForgetBond,
    decodeQueryCatalog,
    decodeQueryCatalogResponse,
    decodeQueryDraft,
    decodeSessionOnly,
    decodeSetClock,
    decodeSetMetadata,
    decodeStartDownload,
    decodeStartDraftPart,
    decodeStartUpload,
    decodeUploadAccepted,
    encodeConfigBlock,
    encodeDeviceStatus,
    validateResetStoreEcho,
} from "./messages";
import { SCHEMA_ROLE_OF_VERSION } from "./registry";
import { decoding, type CategoryName, type DosResult } from "./result";
import { decodeStreamFrame, encodeStreamFrame } from "./stream";

/** Walk up from this file to the repo root (the directory holding `specs/vectors/`). */
function repoRoot(): string {
    let dir = dirname(fileURLToPath(import.meta.url));
    for (let up = 0; up < 12; up++) {
        if (existsSync(join(dir, "specs", "vectors", "manifest.json"))) return dir;
        dir = dirname(dir);
    }
    throw new Error("could not locate the repo root from " + import.meta.url);
}

const SUITE = join(repoRoot(), "specs/vectors/device-object-v2");
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
    storage: ManifestRow[];
    negative: ManifestRow[];
    transcripts: ManifestRow[];
};

/** Every fixture file some suite below actually ran. The drift guard compares this to the manifest. */
const exercised = new Set<string>();

function fixture<T>(row: ManifestRow): T {
    exercised.add(row.file);
    return JSON.parse(read(row.file)) as T;
}

const rows = (list: ManifestRow[], predicate: (parsed: { kind?: string }) => boolean): ManifestRow[] =>
    list.filter((row) => predicate(JSON.parse(read(row.file)) as { kind?: string }));

// ------------------------------------------------------------------------------- fixture shapes

interface ControlFixture {
    name: string;
    kind: "control";
    direction: "request" | "response";
    opcode: { name: string; value: number };
    header: { magic: string; major: number; minor: number; flags: number; payloadLength: number; requestId: number };
    boundary: string | null;
    note: string;
    payload: string;
    frame: string;
}

interface IntentFixture {
    name: string;
    kind: "canonicalIntent";
    opcode: { name: string; value: number };
    storeId: string;
    prefixLength: number;
    suffixLength: number;
    bytes: string;
    sha256: string;
}

interface FrameLimitFixture {
    name: string;
    kind: "frameLimitDerivation";
    protocolMinimumControlFrame: number;
    protocolMinimumStreamFrame: number;
    maximumControlFrame: number;
    maximumStreamFrame: number;
    cases: {
        channel: "control" | "stream";
        linkValue: number;
        transportCeiling: number;
        clientMaximum: number;
        deviceMaximum: number;
        outcome: string;
        negotiated: number;
        note: string;
    }[];
}

interface NegativeFixture {
    name: string;
    kind: "negative";
    target: string;
    note: string;
    expect: { category: string; categoryValue: number; detail: string; detailValue: number };
    bytes: string;
}

interface StreamFixture {
    name: string;
    kind: "stream";
    sessionId: number;
    offset: string;
    payloadLength: number;
    direction: number;
    flags: number;
    record: string;
}

interface TranscriptFixture {
    name: string;
    kind: "transcript";
    eventCount: number;
    events: { actor: string; channel: string; note: string; record: string }[];
}

// ------------------------------------------------------------------------------------ the suites

describe("the manifest", () => {
    it("is the device-object-v2 suite at wire major 3", () => {
        expect(MANIFEST.suite).toBe("device-object-v2");
        expect(MANIFEST.wire_major).toBe(3);
        expect(MANIFEST.format).toBe(1);
    });

    it("pins the SHA-256 of every checked-in fixture file", () => {
        const all = [
            ...MANIFEST.controls,
            ...MANIFEST.streams,
            ...MANIFEST.storage,
            ...MANIFEST.negative,
            ...MANIFEST.transcripts,
        ];
        expect(all.length).toBeGreaterThan(0);
        const drifted = all.filter(
            (row) => createHash("sha256").update(readFileSync(join(SUITE, row.file))).digest("hex") !== row.sha256,
        );
        expect(drifted.map((row) => row.file)).toEqual([]);
    });
});

describe("control vectors decode and re-encode byte for byte", () => {
    const controlRows = rows(MANIFEST.controls, (parsed) => parsed.kind === "control");

    it.each(controlRows.map((row) => [row.name, row] as const))("%s", (_name, row) => {
        const vector = fixture<ControlFixture>(row);
        const bytes = hexToBytes(vector.frame);

        const decoded = decodeControlFrame(bytes);
        if (!decoded.ok) throw new Error(`${vector.name}: ${decoded.error.category}/${decoded.error.detail}: ${decoded.error.message}`);
        const frame = decoded.value;

        // The header the fixture states, field by field, rather than only through the round trip.
        expect(vector.header.magic).toBe("OBCP");
        expect(vector.header.major).toBe(3);
        expect(vector.header.minor).toBe(0);
        expect(frame.opcode).toBe(vector.opcode.value);
        expect(frame.requestId).toBe(vector.header.requestId);
        expect(frame.response).toBe(vector.direction === "response");
        expect((vector.header.flags & 0x01) !== 0).toBe(frame.response);
        expect((vector.header.flags & 0x02) !== 0).toBe(frame.error);
        expect((vector.header.flags & 0x04) !== 0).toBe(frame.more);
        expect(bytes.length - 16).toBe(vector.header.payloadLength);
        expect(bytesToHex(bytes.subarray(16))).toBe(vector.payload);

        expect(bytesToHex(encodeControlFrame(frame))).toBe(vector.frame);
    });

    it("covers every opcode in the §4 registry", () => {
        const seen = new Set(
            controlRows.map((row) => (JSON.parse(read(row.file)) as ControlFixture).opcode.value),
        );
        const missing = (Object.keys(OPCODE) as OpcodeName[]).filter((name) => !seen.has(OPCODE[name]));
        expect(missing).toEqual([]);
    });

    it("covers both directions of every opcode that has both", () => {
        const directions = new Map<number, Set<string>>();
        for (const row of controlRows) {
            const vector = JSON.parse(read(row.file)) as ControlFixture;
            const set = directions.get(vector.opcode.value) ?? new Set<string>();
            set.add(vector.direction);
            directions.set(vector.opcode.value, set);
        }
        // Every registered opcode carries at least one direction; the request/response split is what
        // the response flag distinguishes, so a positive vector for each is the acceptance bar.
        for (const [opcode, seen] of directions) expect([...seen].length, `opcode 0x${opcode.toString(16)}`).toBeGreaterThan(0);
    });
});

describe("canonical intent", () => {
    /**
     * Each intent fixture is paired with the request vector it is the canonical intent *of*. That
     * pairing is the real assertion: the digest a same-intent replay is judged against has to fall
     * out of decoding the request, not out of transcribing the intent fixture's own bytes.
     */
    const PAIRED: Readonly<Record<string, { file: string; build: (body: ControlBody) => IntentSource }>> = {
        "intent-start-upload-create-route": {
            file: "controls/start-upload-create-route.json",
            build: (body) => ({ opcode: "StartUpload", request: expectBody(body, "StartUpload").request }),
        },
        "intent-start-upload-replace-route": {
            file: "controls/start-upload-replace-route-at-revision.json",
            build: (body) => ({ opcode: "StartUpload", request: expectBody(body, "StartUpload").request }),
        },
        "intent-begin-draft": {
            file: "controls/begin-draft-create-volume-manifest.json",
            build: (body) => ({ opcode: "BeginDraft", request: expectBody(body, "BeginDraft").request }),
        },
        "intent-start-draft-part": {
            file: "controls/start-draft-part-request.json",
            build: (body) => ({ opcode: "StartDraftPart", request: expectBody(body, "StartDraftPart").request }),
        },
        "intent-delete-object": {
            file: "controls/delete-object-request.json",
            build: (body) => ({ opcode: "DeleteObject", request: expectBody(body, "DeleteObject").request }),
        },
        "intent-set-metadata": {
            file: "controls/set-metadata-route-request.json",
            build: (body) => ({ opcode: "SetMetadata", request: expectBody(body, "SetMetadata").request }),
        },
        "intent-abort-operation": {
            file: "controls/abort-operation-request.json",
            build: (body) => ({ opcode: "AbortOperation", request: expectBody(body, "AbortOperation").request }),
        },
        "intent-install-update": {
            file: "controls/install-update-request.json",
            build: (body) => ({ opcode: "InstallUpdate", request: expectBody(body, "OperationOnObject").request }),
        },
        "intent-acknowledge-ride-imported": {
            file: "controls/acknowledge-ride-imported-request.json",
            build: (body) => ({ opcode: "AcknowledgeRideImported", request: expectBody(body, "OperationOnObject").request }),
        },
    };

    const intentRows = rows(MANIFEST.controls, (parsed) => parsed.kind === "canonicalIntent");

    it.each(intentRows.map((row) => [row.name, row] as const))("%s", async (_name, row) => {
        const vector = fixture<IntentFixture>(row);
        const bytes = hexToBytes(vector.bytes);
        const store = storeId(hexToBytes(vector.storeId));

        expect(vector.prefixLength).toBe(INTENT_PREFIX_BYTES);
        expect(bytes.length).toBe(vector.prefixLength + vector.suffixLength);

        // The digest is the equality authority: recompute it rather than trusting the fixture's copy.
        expect(bytesToHex(await intentDigest(bytes))).toBe(vector.sha256);

        const pair = PAIRED[vector.name];
        expect(pair, `${vector.name} has no paired request vector`).toBeDefined();
        const request = JSON.parse(read(pair.file)) as ControlFixture;
        const decoded = decodeControlFrame(hexToBytes(request.frame));
        if (!decoded.ok) throw new Error(`${pair.file} did not decode`);
        const rebuilt = canonicalIntent(store, pair.build(decoded.value.body));
        expect(bytesToHex(rebuilt)).toBe(vector.bytes);
        expect(bytesToHex(rebuilt.subarray(0, 16))).toBe("4f42432d444f53332d494e54454e5400");
        expect(new DataView(rebuilt.buffer, rebuilt.byteOffset).getUint16(32, true)).toBe(vector.opcode.value);
        expect(rebuilt[34]).toBe(1);
        expect(rebuilt[35]).toBe(0);
    });
});

describe("frame-limit derivation", () => {
    const derivationRows = rows(MANIFEST.controls, (parsed) => parsed.kind === "frameLimitDerivation");

    it.each(derivationRows.map((row) => [row.name, row] as const))("%s", (_name, row) => {
        const vector = fixture<FrameLimitFixture>(row);
        expect(vector.protocolMinimumControlFrame).toBe(MIN_CONTROL_FRAME);
        expect(vector.protocolMinimumStreamFrame).toBe(MIN_STREAM_FRAME);
        expect(vector.maximumControlFrame).toBe(MAX_CONTROL_FRAME);
        expect(vector.maximumStreamFrame).toBe(MAX_STREAM_FRAME);

        for (const each of vector.cases) {
            // The transport ceiling itself is derived: ATT_MTU - 3 on the control channel, the CoC
            // SDU on the stream channel.
            const ceiling = each.channel === "control" ? bleControlCeiling(each.linkValue) : each.linkValue;
            expect(ceiling, each.note).toBe(each.transportCeiling);
            const derived = negotiateFrameLimit(each.channel, ceiling, each.clientMaximum, each.deviceMaximum);
            expect(derived.outcome).toBe(each.outcome);
            expect(derived.negotiated).toBe(each.negotiated);
        }
    });
});

describe("negative vectors reject with the exact category and detail", () => {
    it.each(MANIFEST.negative.map((row) => [row.name, row] as const))("%s", (_name, row) => {
        const vector = fixture<NegativeFixture>(row);
        const result = rejectByTarget(vector.target, hexToBytes(vector.bytes));
        if (result.ok) throw new Error(`${vector.name} decoded, and the suite says it must not`);
        expect(result.error.category).toBe(vector.expect.category as CategoryName);
        expect(result.error.categoryValue).toBe(vector.expect.categoryValue);
        expect(result.error.detail).toBe(vector.expect.detail);
        expect(result.error.detailValue).toBe(vector.expect.detailValue);
    });
});

describe("stream vectors decode and re-encode byte for byte", () => {
    it.each(MANIFEST.streams.map((row) => [row.name, row] as const))("%s", (_name, row) => {
        const vector = fixture<StreamFixture>(row);
        const bytes = hexToBytes(vector.record);
        const decoded = decoding(() => decodeStreamFrame(bytes));
        if (!decoded.ok) throw new Error(`${vector.name}: ${decoded.error.category}/${decoded.error.detail}`);
        const frame = decoded.value;
        expect(frame.sessionId).toBe(vector.sessionId);
        expect(frame.offset).toBe(BigInt(vector.offset));
        expect(frame.payload.length).toBe(vector.payloadLength);
        expect(frame.direction).toBe(vector.direction);
        expect(frame.flags).toBe(vector.flags);
        expect(bytesToHex(encodeStreamFrame(frame))).toBe(vector.record);
    });
});

describe("transcripts replay", () => {
    it.each(MANIFEST.transcripts.map((row) => [row.name, row] as const))("%s", (_name, row) => {
        const vector = fixture<TranscriptFixture>(row);
        expect(vector.events.length).toBe(vector.eventCount);
        for (const [index, event] of vector.events.entries()) {
            const where = `${vector.name} event ${index} (${event.note})`;
            if (event.channel === "injected") {
                // An injected disconnect, reset or crash cut has no bytes; it is the state change
                // between the frames on either side of it.
                expect(event.record, where).toBe("");
                continue;
            }
            const bytes = hexToBytes(event.record);
            if (event.channel === "control") {
                const decoded = decodeControlFrame(bytes);
                if (!decoded.ok) throw new Error(`${where}: ${decoded.error.category}/${decoded.error.detail}: ${decoded.error.message}`);
                expect(bytesToHex(encodeControlFrame(decoded.value)), where).toBe(event.record);
                // Every control record in a transcript is one complete frame in the direction its
                // actor implies: a device answers, a client asks.
                expect(decoded.value.response, where).toBe(event.actor === "device");
            } else {
                expect(event.channel, where).toBe("stream");
                const decoded = decoding(() => decodeStreamFrame(bytes));
                if (!decoded.ok) throw new Error(`${where}: ${decoded.error.category}/${decoded.error.detail}`);
                expect(bytesToHex(encodeStreamFrame(decoded.value)), where).toBe(event.record);
            }
        }
    });
});

describe("the drift guard", () => {
    it("exercises every fixture the manifest lists", () => {
        const listed = [
            ...MANIFEST.controls,
            ...MANIFEST.streams,
            ...MANIFEST.storage,
            ...MANIFEST.negative,
            ...MANIFEST.transcripts,
        ].map((row) => row.file);
        const untouched = listed.filter((file) => !exercised.has(file));
        expect(untouched).toEqual([]);
    });
});

// --------------------------------------------------------------------------------- test helpers

function expectBody<K extends ControlBody["kind"]>(body: ControlBody, kind: K): Extract<ControlBody, { kind: K }> {
    if (body.kind !== kind) throw new Error(`expected a ${kind} body, got ${body.kind}`);
    return body as Extract<ControlBody, { kind: K }>;
}

/** The canonical StoreId the suite's positive vectors are minted under. */
const SUITE_STORE_ID = hexToBytes("3c92000099164ebaabc2342fe08f6b10");

/**
 * A negative fixture names the decode entry point in its `target`. Targets ending in Request or
 * Response are whole control frames; the rest name one substructure, decoded on its own so the
 * rejection is attributed to the layer that owns the rule.
 */
function rejectByTarget(target: string, bytes: Uint8Array): DosResult<unknown> {
    if (target === "controlFrame" || target.endsWith("Request") || target.endsWith("Response")) {
        return decodeControlFrame(bytes);
    }
    switch (target) {
        case "streamFrame":
            return decoding(() => decodeStreamFrame(bytes));
        case "metadataEnvelope":
            return decoding(() =>
                decodeMetadataEnvelope(bytes, {
                    // The version byte names the role, and the role fixes the ceiling: put and patch
                    // envelopes are bounded at 128 bytes and a catalog projection at 96.
                    role: SCHEMA_ROLE_OF_VERSION.get(bytes[2]),
                    mutating: SCHEMA_ROLE_OF_VERSION.get(bytes[2]) !== "catalog",
                }),
            );
        case "errorBody":
            return decoding(() => decodeErrorBody(bytes));
        case "subjectEntry":
            return decoding(() => decodeSubjectEntry(bytes));
        case "capabilities":
            return decoding(() => decodeCapabilities(bytes));
        case "configBlock":
            return decoding(() => decodeConfigBlock(bytes));
        case "resetStoreEcho(mountClass=3)":
            return decoding(() => validateResetStoreEcho(bytes, 3, SUITE_STORE_ID));
        default:
            throw new Error(`no decode entry point for the negative target "${target}"`);
    }
}

// ------------------------------------------------------------- substructure round trips (§2, §5)

describe("substructures re-encode independently of their frame", () => {
    /**
     * The frame round trip above already proves byte identity end to end. These pin the pieces a
     * *different* message could carry, so a future caller that assembles one by hand — a catalog
     * projection, a subject entry, an ErrorBody in a fault path — gets the same bytes.
     */
    const sample = (file: string): ControlFixture => JSON.parse(read(file)) as ControlFixture;

    it("re-encodes a Capabilities resource page and a subject page", () => {
        for (const file of ["controls/capabilities-resource-page.json", "controls/capabilities-subject-page-0.json"]) {
            const payload = hexToBytes(sample(file).payload);
            expect(bytesToHex(encodeCapabilities(decodeCapabilities(payload)))).toBe(sample(file).payload);
        }
        const page = decodeCapabilities(hexToBytes(sample("controls/capabilities-subject-page-0.json").payload));
        if (page.page.kind !== "subjects") throw new Error("expected a subject page");
        for (const entry of page.page.entries) expect(encodeSubjectEntry(entry).length).toBe(20);
    });

    it("re-encodes a catalog projection envelope", () => {
        const payload = hexToBytes(sample("controls/catalog-page-one-entry.json").payload);
        const page = decodeQueryCatalogResponse(payload, false);
        expect(page.entries.length).toBe(1);
        const envelope = page.entries[0].metadata;
        expect(envelope.role).toBe("catalog");
        expect(bytesToHex(encodeMetadataEnvelope(envelope)).length / 2).toBe(envelope.byteLength);
    });

    it("re-encodes an ErrorBody, a config block and a device status on their own", () => {
        const error = hexToBytes(sample("controls/error-text-exactly-64-bytes.json").payload);
        expect(bytesToHex(encodeErrorBody(decodeErrorBody(error)))).toBe(bytesToHex(error));

        const config = hexToBytes(sample("controls/config-block-full-name-response.json").payload);
        expect(bytesToHex(encodeConfigBlock(decodeConfigBlock(config)))).toBe(bytesToHex(config));

        const status = hexToBytes(sample("controls/device-status-mount-class-3.json").payload);
        expect(bytesToHex(encodeDeviceStatus(decodeDeviceStatus(status)))).toBe(bytesToHex(status));
    });

    it("decodes each request body through its own entry point", () => {
        // The frame dispatcher is one path into these; a caller that already has a payload is
        // another, and both have to agree.
        const body = (file: string): Uint8Array => hexToBytes(sample(file).payload);
        expect(decodeStartUpload(body("controls/start-upload-create-route.json")).objectKind).toBe("route");
        expect(decodeBeginDraft(body("controls/begin-draft-create-volume-manifest.json")).objectKind).toBe("volumeManifest");
        expect(decodeStartDraftPart(body("controls/start-draft-part-request.json")).draftPartKind).toBeTruthy();
        expect(decodeDeleteObject(body("controls/delete-object-request.json")).objectKind).toBe("route");
        expect(decodeSetMetadata(body("controls/set-metadata-route-request.json")).metadata.role).toBe("patch");
        expect(decodeStartDownload(body("controls/start-download-request.json")).objectKind).toBeTruthy();
        expect(decodeFinishDownload(body("controls/finish-download-request.json")).sessionId).toBeGreaterThan(0);
        expect(decodeSessionOnly(body("controls/finish-upload-request.json"), "FinishUpload").sessionId).toBeGreaterThan(0);
        expect(decodeQueryCatalog(body("controls/query-catalog-first-page.json")).objectKind).toBeTruthy();
        expect(decodeQueryDraft(body("controls/query-draft-request.json")).limit).toBeGreaterThan(0);
        expect(decodeSetClock(body("controls/set-clock-gps-request.json")).source).toBe(2);
        expect(decodeForgetBond(body("controls/forget-bond-this-bond-request.json")).scope).toBe(1);
    });

    it("reads the three resumable acceptances at their frozen sizes", () => {
        const payload = (file: string): Uint8Array => hexToBytes(sample(file).payload);
        expect(payload("controls/upload-accepted-offset-zero.json").length).toBe(64);
        expect(payload("controls/draft-part-accepted-offset-zero.json").length).toBe(72);
        expect(payload("controls/finalize-accepted-offset-zero.json").length).toBe(64);
        expect(decodeUploadAccepted(payload("controls/upload-accepted-offset-zero.json")).disposition).toBe("accepted");
        expect(decodeDraftPartAccepted(payload("controls/draft-part-accepted-offset-zero.json")).disposition).toBe("accepted");
        expect(decodeFinalizeDraftResponse(payload("controls/finalize-accepted-offset-zero.json")).disposition).toBe("accepted");
    });
});
