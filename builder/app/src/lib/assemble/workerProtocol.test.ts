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
                inputOnDisk: true,
                streamedShardBytes: 256 * 1024 * 1024,
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
    it("moves a file's buffer and nothing else", () => {
        const bytes = new Uint8Array([9]);
        const file: AssembleWorkerResponse = {
            type: "file",
            name: "MS1S00.OBM",
            role: "core",
            sha256: "0".repeat(64),
            byteLength: 1,
            bytes,
        };
        expect(responseTransferList(file)).toEqual([bytes.buffer]);
        expect(responseTransferList({ type: "progress", phase: "nav", fraction: 0.5 })).toEqual([]);
    });

    /** A streamed shard was evicted from wasm memory to keep the peak down;
     *  copying it across the port would put it straight back. */
    it("moves a streamed shard's buffer too", () => {
        const bytes = new Uint8Array([7]);
        expect(
            responseTransferList({
                type: "shard",
                name: "MS1S00.OBM",
                role: "core",
                sha256: "0".repeat(64),
                byteLength: 1,
                bytes,
            }),
        ).toEqual([bytes.buffer]);
    });
});

describe("isWorkerResponse", () => {
    it("accepts every message the worker actually sends", () => {
        const messages: AssembleWorkerResponse[] = [
            { type: "progress", phase: "verify", fraction: 0.9 },
            {
                type: "planned",
                totalBytes: 129,
                shardCount: 1,
                warnings: [],
                summary: { cells: 1, bytes: 1, manifest: "MS1.OBS", shards: [] },
            },
            {
                type: "file",
                name: "MS1.OBS",
                role: "manifest",
                sha256: "",
                byteLength: 128,
                bytes: new Uint8Array(1),
            },
            {
                type: "shard",
                name: "MS1S00.OBM",
                role: "core",
                sha256: "a".repeat(64),
                byteLength: 4,
                bytes: new Uint8Array(4),
            },
            { type: "reading", mode: "streamed", cells: 412 },
            { type: "done", warnings: [], summary: { cells: 1, bytes: 2, manifest: "MS1.OBS", shards: [] } },
            {
                type: "estimate-result",
                estimate: est(true),
                deviceEstimate: est(false),
            },
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
        expect(isWorkerResponse({ type: "reading", mode: "telepathy", cells: 1 })).toBe(false);
        // Both verdicts or neither: a one-estimate answer is a stale worker build, and letting it
        // through would leave the device gate reading `null` as "fits".
        expect(isWorkerResponse({ type: "estimate-result", estimate: est(true) })).toBe(false);
        expect(isWorkerResponse({ type: "error", code: "not-a-code", message: "x" })).toBe(false);
        expect(isWorkerResponse({ type: "file", name: "x", role: "core", sha256: "", byteLength: 1, bytes: [1] })).toBe(
            false,
        );
        // Only OBCM shards are streamed: the terrain shard and the manifest are
        // never evicted, so a `shard` claiming to be one is not this protocol.
        expect(
            isWorkerResponse({
                type: "shard",
                name: "MS1.OBS",
                role: "manifest",
                sha256: "",
                byteLength: 1,
                bytes: new Uint8Array(1),
            }),
        ).toBe(false);
    });
});
