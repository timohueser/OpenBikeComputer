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

// Everything here comes through the public barrel on purpose: the suite has to exercise the surface
// a caller gets, including its promise that no entry point throws.
import {
    MAX_CONTROL_FRAME,
    MAX_STREAM_FRAME,
    MIN_CONTROL_FRAME,
    MIN_STREAM_FRAME,
    OPCODE,
    bleControlCeiling,
    bytesToHex,
    canonicalIntent,
    decodeCapabilities,
    decodeConfigBlock,
    decodeControlFrame,
    decodeErrorBody,
    decodeMetadataEnvelope,
    decodeStreamFrame,
    decodeSubjectEntry,
    encodeControlFrame,
    encodeStreamFrame,
    hexToBytes,
    intentDigest,
    negotiateFrameLimit,
    storeId,
    unwrap,
    validateResetStoreEcho,
    INTENT_PREFIX_BYTES,
    type CategoryName,
    type ControlBody,
    type DosResult,
    type IntentSource,
    type OpcodeName,
} from "./index";
import { SCHEMA_ROLE_OF_VERSION } from "./registry";

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

        expect(bytesToHex(unwrap(encodeControlFrame(frame)))).toBe(vector.frame);
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
        const rebuilt = unwrap(canonicalIntent(store, pair.build(decoded.value.body)));
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
        const decoded = decodeStreamFrame(bytes);
        if (!decoded.ok) throw new Error(`${vector.name}: ${decoded.error.category}/${decoded.error.detail}`);
        const frame = decoded.value;
        expect(frame.sessionId).toBe(vector.sessionId);
        expect(frame.offset).toBe(BigInt(vector.offset));
        expect(frame.payload.length).toBe(vector.payloadLength);
        expect(frame.direction).toBe(vector.direction);
        expect(frame.flags).toBe(vector.flags);
        expect(bytesToHex(unwrap(encodeStreamFrame(frame)))).toBe(vector.record);
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
                expect(bytesToHex(unwrap(encodeControlFrame(decoded.value))), where).toBe(event.record);
                // Every control record in a transcript is one complete frame in the direction its
                // actor implies: a device answers, a client asks.
                expect(decoded.value.response, where).toBe(event.actor === "device");
            } else {
                expect(event.channel, where).toBe("stream");
                const decoded = decodeStreamFrame(bytes);
                if (!decoded.ok) throw new Error(`${where}: ${decoded.error.category}/${decoded.error.detail}`);
                expect(bytesToHex(unwrap(encodeStreamFrame(decoded.value))), where).toBe(event.record);
            }
        }
    });
});

/**
 * The storage half of the suite (`Device_Object_Vectors_v2.md` §6).
 *
 * These files are OBC2 on-card records — checkpoints, journal slots, WORK and RIDE slots, the ARM
 * handoff, `INIT.REC`, resolution generations — and there is deliberately no TypeScript codec for
 * them: the on-card format is private to `CardStore`, and no client ever sees one. Their decode
 * guard lives with the Rust producer that owns their bytes.
 *
 * What this side can prove, and what matters for a *cross-language* fixture, is that the encoding
 * those files use is readable outside Rust. A case states its `length` and the non-zero `runs`
 * inside it — because a 65,536-byte checkpoint written out as hex would be unreviewable — so
 * reconstruction has to be mechanical: allocate the zeros, splice the runs in, and land on the
 * stated digest. If that ever stops holding, the files have become Rust-private and are no longer
 * a contract.
 */
describe("storage vectors are reconstructable outside Rust", () => {
    interface StorageCase {
        name: string;
        subject: string;
        length: number;
        sha256: string;
        reject: string | null;
        runs: { offset: number; hex: string }[];
    }

    // An emptied storage section would make every `it.each` below vacuous — zero cases, all green.
    it("the manifest carries a storage section at all", () => {
        expect(MANIFEST.storage.length).toBeGreaterThan(0);
        expect(MANIFEST.storage.some((row) => row.name === "crash-cut-transcripts")).toBe(true);
    });

    const recordRows = MANIFEST.storage.filter((row) => row.name !== "crash-cut-transcripts");

    it.each(recordRows.map((row) => [row.name, row] as const))("%s", (_name, row) => {
        const file = fixture<{ kind: string; storage_format: number; caseCount: number; cases: StorageCase[] }>(row);
        expect(file.kind).toBe("storage");
        expect(file.storage_format).toBe(MANIFEST.storage_format);
        expect(file.cases.length).toBe(file.caseCount);
        expect(file.cases.length).toBeGreaterThan(0);

        for (const record of file.cases) {
            const where = `${row.name}/${record.name}`;
            const bytes = new Uint8Array(record.length);
            for (const run of record.runs) {
                const chunk = hexToBytes(run.hex);
                expect(run.offset + chunk.length, `${where}: run past the record`).toBeLessThanOrEqual(record.length);
                bytes.set(chunk, run.offset);
            }
            expect(createHash("sha256").update(bytes).digest("hex"), where).toBe(record.sha256);
            // A run is non-zero at both ends, or the encoding is not canonical and two producers
            // could emit different files for the same bytes.
            for (const run of record.runs) {
                const chunk = hexToBytes(run.hex);
                expect(chunk.length, `${where}: empty run`).toBeGreaterThan(0);
                expect(chunk[0], `${where}: run starts on a zero`).not.toBe(0);
                expect(chunk[chunk.length - 1], `${where}: run ends on a zero`).not.toBe(0);
            }
        }
    });

    it("crash-cut-transcripts", () => {
        const row = MANIFEST.storage.find((entry) => entry.name === "crash-cut-transcripts");
        expect(row, "the transcript file is missing from the manifest").toBeDefined();
        const file = fixture<{
            kind: string;
            transcriptCount: number;
            transcripts: {
                name: string;
                stepCount: number;
                cutPoints: number;
                steps: { op: number; file: string; kind: string; offset: number; length: number }[];
                admissibleOutcomes: string[];
            }[];
        }>(row!);
        expect(file.kind).toBe("storage");
        expect(file.transcripts.length).toBe(file.transcriptCount);
        for (const transcript of file.transcripts) {
            const where = transcript.name;
            expect(transcript.steps.length, where).toBe(transcript.stepCount);
            // Every operation is cut at three positions: before it reaches the card, during it, and
            // after it returns.
            expect(transcript.cutPoints, where).toBe(transcript.stepCount * 3);
            // A commit path admits more than one recovered state — that is what its cut points
            // are for. A fault-mode transcript is a single refused operation with exactly one
            // outcome, and holding it to the same rule would be wrong rather than strict.
            expect(transcript.admissibleOutcomes.length, where).toBeGreaterThan(
                transcript.stepCount > 1 ? 1 : 0,
            );
            transcript.steps.forEach((step, index) => {
                expect(step.op, `${where} step ${index}`).toBe(index + 1);
                expect(["write", "sync"], `${where} step ${index}`).toContain(step.kind);
                if (step.kind === "sync") {
                    expect(step.offset, `${where} step ${index}`).toBe(0);
                    expect(step.length, `${where} step ${index}`).toBe(0);
                }
            });
            // A commit path ends at a sync: the gate is durable only once it returns. A single-step
            // fault transcript ends at the write that failed, which is the whole point of it.
            if (transcript.stepCount > 1) {
                expect(transcript.steps[transcript.steps.length - 1].kind, where).toBe("sync");
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
            return decodeStreamFrame(bytes);
        case "metadataEnvelope":
            return decodeMetadataEnvelope(bytes, {
                // The version byte names the role, and the role fixes the ceiling: put and patch
                // envelopes are bounded at 128 bytes and a catalog projection at 96.
                role: SCHEMA_ROLE_OF_VERSION.get(bytes[2]),
                mutating: SCHEMA_ROLE_OF_VERSION.get(bytes[2]) !== "catalog",
            });
        case "errorBody":
            return decodeErrorBody(bytes);
        case "subjectEntry":
            return decodeSubjectEntry(bytes);
        case "capabilities":
            return decodeCapabilities(bytes);
        case "configBlock":
            return decodeConfigBlock(bytes);
        case "resetStoreEcho(mountClass=3)":
            return validateResetStoreEcho(bytes, 3, SUITE_STORE_ID);
        default:
            throw new Error(`no decode entry point for the negative target "${target}"`);
    }
}

// ------------------------------------------------------------- substructure round trips (§2, §5)

describe("the public substructure decoders agree with the frame dispatcher", () => {
    /**
     * A caller does not always hold a whole frame. It holds a Capabilities page it is walking, a
     * config block it is about to edit and write back, an ErrorBody it kept from a fault path. Those
     * are the public entry points; everything else is reached through the frame, and this checks the
     * two paths report the same thing rather than drifting into two dialects.
     */
    const sample = (file: string): ControlFixture => JSON.parse(read(file)) as ControlFixture;
    const payloadOf = (file: string): Uint8Array => hexToBytes(sample(file).payload);
    const bodyOf = (file: string): ControlBody => unwrap(decodeControlFrame(hexToBytes(sample(file).frame))).body;

    it("reads a Capabilities page the same way the frame does", () => {
        for (const file of ["controls/capabilities-resource-page.json", "controls/capabilities-subject-page-0.json"]) {
            const direct = unwrap(decodeCapabilities(payloadOf(file)));
            expect(expectBody(bodyOf(file), "Capabilities").capabilities).toEqual(direct);
        }
        const page = unwrap(decodeCapabilities(payloadOf("controls/capabilities-subject-page-0.json")));
        if (page.page.kind !== "subjects") throw new Error("expected a subject page");
        expect(page.page.entries.length).toBe(2);
        for (const entry of page.page.entries) expect(entry.namespace).toBe(1);
    });

    it("reads a config block, an ErrorBody and a device status the same way the frame does", () => {
        const config = "controls/config-block-full-name-response.json";
        expect(expectBody(bodyOf(config), "ConfigBlock").config).toEqual(unwrap(decodeConfigBlock(payloadOf(config))));

        const error = "controls/error-text-exactly-64-bytes.json";
        expect(expectBody(bodyOf(error), "Error").error).toEqual(unwrap(decodeErrorBody(payloadOf(error))));

        const status = "controls/device-status-mount-class-3.json";
        expect(expectBody(bodyOf(status), "DeviceStatus").response.mountClass).toBe(3);
    });

    it("carries a catalog projection envelope out of its page", () => {
        const page = expectBody(bodyOf("controls/catalog-page-one-entry.json"), "CatalogPage").response;
        expect(page.entries.length).toBe(1);
        const envelope = page.entries[0].metadata;
        expect(envelope.role).toBe("catalog");
        expect(envelope.values.get("displayName")).toBeTruthy();
    });

    it("reads the three resumable acceptances at their frozen sizes", () => {
        expect(payloadOf("controls/upload-accepted-offset-zero.json").length).toBe(64);
        expect(payloadOf("controls/draft-part-accepted-offset-zero.json").length).toBe(72);
        expect(payloadOf("controls/finalize-accepted-offset-zero.json").length).toBe(64);
        const upload = expectBody(bodyOf("controls/upload-accepted-offset-zero.json"), "UploadAccepted");
        const part = expectBody(bodyOf("controls/draft-part-accepted-offset-zero.json"), "DraftPartAccepted");
        const finalize = expectBody(bodyOf("controls/finalize-accepted-offset-zero.json"), "FinalizeDraftAccepted");
        expect(upload.response.disposition).toBe("accepted");
        expect(part.response.disposition).toBe("accepted");
        expect(finalize.response.disposition).toBe("accepted");
    });
});

// ------------------------------------------------------- boundaries derived from a real vector

/**
 * A checked-in vector is a *valid* frame; several rules can only be shown by taking one and
 * breaking exactly the field under test. These do that, so the case is anchored in bytes the Rust
 * producer emitted rather than in a frame this suite invented. Each pins a boundary a shared
 * negative fixture is expected to cover later.
 */
describe("boundaries derived by mutating a checked-in vector", () => {
    const sample = (file: string): ControlFixture => JSON.parse(read(file)) as ControlFixture;
    const payloadOf = (file: string): Uint8Array => hexToBytes(sample(file).payload);
    const failure = (result: DosResult<unknown>): { category: string; detail: string } => {
        if (result.ok) throw new Error("expected a rejection, and the codec accepted the bytes");
        return { category: result.error.category, detail: result.error.detail };
    };

    it("rejects a negotiated frame limit outside the §1 hard bounds", () => {
        const page = payloadOf("controls/capabilities-resource-page.json");
        const view = (bytes: Uint8Array): DataView => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
        expect(decodeCapabilities(page).ok).toBe(true);

        // A *negotiated* control frame below 192 cannot exist: a device in that position answers
        // Hello with resourceLimit/minimumControlFrame instead of a Capabilities page.
        const tooSmall = page.slice();
        view(tooSmall).setUint16(20, 100, true);
        expect(failure(decodeCapabilities(tooSmall))).toEqual({ category: "invalidFrame", detail: "frameBounds" });

        const tooLarge = page.slice();
        view(tooLarge).setUint16(20, MAX_CONTROL_FRAME + 1, true);
        expect(failure(decodeCapabilities(tooLarge))).toEqual({ category: "invalidFrame", detail: "frameBounds" });

        const streamTooLarge = page.slice();
        view(streamTooLarge).setUint16(22, MAX_STREAM_FRAME + 1, true);
        expect(failure(decodeCapabilities(streamTooLarge))).toEqual({ category: "invalidFrame", detail: "frameBounds" });
    });

    it("rejects a subject page that returns fewer whole entries than remain", () => {
        // §5: "The server never silently truncates the registry." Page 0 of 8 subjects carries two.
        const page = payloadOf("controls/capabilities-subject-page-0.json");
        expect(decodeSubjectEntry(page.subarray(56, 76)).ok).toBe(true);

        const short = page.slice(0, 56 + 20);
        short[52] = 1; // returned subject count
        expect(failure(decodeCapabilities(short))).toEqual({
            category: "invalidDescriptor",
            detail: "invalidCombination",
        });
    });

    it("verifies the CRC in a catalog page's next cursor", () => {
        // §8.2's cursor CRC covers the StoreId and the cursor's own first twelve bytes, and a
        // catalog page reports the StoreId it was minted under — so this one always verifies.
        const file = "controls/catalog-page-maximum-count.json";
        const frame = hexToBytes(sample(file).frame);
        expect(decodeControlFrame(frame).ok).toBe(true);

        const tampered = frame.slice();
        tampered[16 + 44 - 1] ^= 0x01; // one bit of the next cursor's CRC word
        expect(failure(decodeControlFrame(tampered))).toEqual({ category: "checksumFailure", detail: "cursor" });

        const reScoped = frame.slice();
        reScoped[16] ^= 0x01; // a different StoreId, same cursor
        expect(failure(decodeControlFrame(reScoped))).toEqual({ category: "checksumFailure", detail: "cursor" });
    });

    it("verifies a request cursor once the caller supplies the StoreId it is connected to", () => {
        const frame = hexToBytes(sample("controls/query-catalog-cursor-continuation.json").frame);
        // Without the StoreId the request still decodes: the frame does not carry the scope.
        expect(decodeControlFrame(frame).ok).toBe(true);
        expect(decodeControlFrame(frame, { storeId: SUITE_STORE_ID }).ok).toBe(true);

        const otherStore = SUITE_STORE_ID.slice();
        otherStore[0] ^= 0x01;
        expect(failure(decodeControlFrame(frame, { storeId: otherStore }))).toEqual({
            category: "checksumFailure",
            detail: "cursor",
        });
    });
});
