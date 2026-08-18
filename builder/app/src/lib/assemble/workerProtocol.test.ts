import { describe, expect, it } from "vitest";
import {
    isWorkerResponse,
    requestTransferList,
    responseTransferList,
    type AssembleWorkerRequest,
    type AssembleWorkerResponse,
    type WorkerCell,
} from "./workerProtocol";

function assembleReq(cells: WorkerCell[]): Extract<AssembleWorkerRequest, { type: "assemble" }> {
    return { type: "assemble", cells, knownEmpty: [], schemaJson: "{}", skinJson: "{}", options: {} };
}

/** A structurally complete estimate, since the validator only checks the object's presence. */
function est(fits: boolean) {
    return {
        engineBytes: 1,
        inputBytes: 1,
        outputBytes: 1,
        peakBytes: 3,
        budgetBytes: 4,
        ceilingBytes: 5,
        headroomBytes: fits ? 1 : -1,
        fits,
    };
}

/** A structurally complete summary, since the validator only checks the object's presence. */
function summary() {
    return { cells: 1, bytes: 2, sha256: "e".repeat(64), verified: null };
}

describe("requestTransferList", () => {
    it("moves every cell's buffer, once each", () => {
        const a = new Uint8Array([1, 2, 3]);
        const b = new Uint8Array([4]);
        const list = requestTransferList(
            assembleReq([
                { id: "18/0001/0001", band: "fine", partial: false, bytes: a },
                { id: "18/0001/0002", band: "fine", partial: false, bytes: b },
            ]),
        );
        expect(list).toHaveLength(2);
        expect(list).toContain(a.buffer);
        expect(list).toContain(b.buffer);
    });

    it("lists a shared buffer once — transferring it twice would throw", () => {
        const pool = new ArrayBuffer(8);
        const list = requestTransferList(
            assembleReq([
                { id: "18/0001/0001", band: "fine", partial: false, bytes: new Uint8Array(pool, 0, 4) },
                { id: "18/0001/0002", band: "fine", partial: false, bytes: new Uint8Array(pool, 4, 4) },
            ]),
        );
        expect(list).toEqual([pool]);
    });

    it("transfers nothing for an estimate — there are no bytes in it", () => {
        expect(
            requestTransferList({
                type: "estimate",
                networkBandBytes: 1,
                totalCellBytes: 2,
                terrainBytes: 0,
                onDisk: true,
                mergeBudgetBytes: 256 * 1024 * 1024,
            }),
        ).toEqual([]);
    });

    it("does not invent a transferable for a known-empty identity", () => {
        const req = assembleReq([]);
        req.knownEmpty.push({ id: "18/0001/0001", band: "fine" });
        expect(requestTransferList(req)).toEqual([]);
    });
});

describe("responseTransferList", () => {
    it("moves the map's buffer and nothing else", () => {
        const bytes = new Uint8Array([9]);
        const file: AssembleWorkerResponse = {
            type: "file",
            sha256: "0".repeat(64),
            byteLength: 1,
            bytes,
        };
        expect(responseTransferList(file)).toEqual([bytes.buffer]);
        expect(responseTransferList({ type: "progress", phase: "nav", fraction: 0.5 })).toEqual([]);
    });

    /** Nothing crosses the port for a sunk map — the bytes are in OPFS, which is the whole point.
     *  A transfer list that invented one would throw at `postMessage`. */
    it("transfers nothing for a map that stayed on disk", () => {
        expect(
            responseTransferList({
                type: "stored-map",
                sha256: "d".repeat(64),
                byteLength: 2_800_000_000,
            }),
        ).toEqual([]);
    });
});

describe("isWorkerResponse", () => {
    it("accepts every message the worker actually sends", () => {
        const messages: AssembleWorkerResponse[] = [
            { type: "progress", phase: "verify", fraction: 0.9 },
            { type: "file", sha256: "a".repeat(64), byteLength: 128, bytes: new Uint8Array(1) },
            { type: "reading", mode: "streamed", cells: 412 },
            { type: "writing", mode: "disk" },
            // Past 2 GiB on purpose: a sunk map is the case this message exists for, and it is
            // routinely larger than anything that could have crossed the port.
            { type: "stored-map", sha256: "b".repeat(64), byteLength: 8_800_000_000 },
            { type: "done", warnings: [], summary: summary() },
            { type: "estimate-result", estimate: est(true) },
            { type: "error", code: "capacity", message: "too big" },
        ];
        for (const msg of messages) expect(isWorkerResponse(msg)).toBe(true);
    });

    /** A cell named by key carries no buffer, so there is nothing to transfer — and a request that
     *  mistakenly listed one would throw at `postMessage`. */
    it("transfers nothing for cells that stayed on disk", () => {
        const req = assembleReq([]);
        req.cellStore = "r0123456789abcdef";
        req.sourceCells = [
            { id: "18/0001/0001", band: "fine", partial: false, byteLength: 4096, key: "a".repeat(64) },
        ];
        expect(requestTransferList(req)).toEqual([]);
    });

    it("rejects strays instead of letting them drive the download screen", () => {
        expect(isWorkerResponse(null)).toBe(false);
        expect(isWorkerResponse({})).toBe(false);
        expect(isWorkerResponse({ type: "progress", phase: "warp", fraction: 0.1 })).toBe(false);
        // `manifest` was a phase when a map was a set; a worker still sending it is a stale build.
        expect(isWorkerResponse({ type: "progress", phase: "manifest", fraction: 0.1 })).toBe(false);
        expect(isWorkerResponse({ type: "reading", mode: "telepathy", cells: 1 })).toBe(false);
        expect(isWorkerResponse({ type: "writing", mode: "telepathy" })).toBe(false);
        expect(isWorkerResponse({ type: "estimate-result", estimate: null })).toBe(false);
        expect(isWorkerResponse({ type: "error", code: "not-a-code", message: "x" })).toBe(false);
        // A `file` whose bytes are not bytes: the one field the download screen dereferences.
        expect(isWorkerResponse({ type: "file", sha256: "a".repeat(64), byteLength: 1, bytes: [1] })).toBe(false);
        // The message variants a set-era worker would send. There is no shard, no manifest and no
        // plan any more, and a build still speaking them must be dropped rather than half-understood.
        expect(
            isWorkerResponse({ type: "shard", name: "MS1S00.OBM", role: "core", sha256: "", byteLength: 1 }),
        ).toBe(false);
        expect(isWorkerResponse({ type: "stored-shard", role: "core", sha256: "", byteLength: 1, entry: "s00.part" })).toBe(
            false,
        );
        expect(isWorkerResponse({ type: "planned", totalBytes: 1, shardCount: 1, warnings: [], summary: summary() })).toBe(
            false,
        );
    });

    /**
     * The map's identity is what both output messages are *for*, so a half-built one must be
     * dropped rather than shown. An empty digest or a fractional/negative length is a stale or
     * broken producer, and the screen would otherwise report a map that nothing can identify.
     */
    it("rejects an output message whose identity is not one", () => {
        const stored = { type: "stored-map", sha256: "c".repeat(64), byteLength: 4 };
        expect(isWorkerResponse(stored)).toBe(true);
        expect(isWorkerResponse({ ...stored, sha256: "" })).toBe(false);
        expect(isWorkerResponse({ ...stored, sha256: undefined })).toBe(false);
        expect(isWorkerResponse({ ...stored, byteLength: -1 })).toBe(false);
        expect(isWorkerResponse({ ...stored, byteLength: 1.5 })).toBe(false);
        expect(isWorkerResponse({ ...stored, byteLength: undefined })).toBe(false);
        // …and the same shape rules bind the buffered arm, which carries it too.
        const file = { type: "file", sha256: "c".repeat(64), byteLength: 4, bytes: new Uint8Array(4) };
        expect(isWorkerResponse(file)).toBe(true);
        expect(isWorkerResponse({ ...file, sha256: "" })).toBe(false);
        expect(isWorkerResponse({ ...file, byteLength: -1 })).toBe(false);
    });
});
