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
        expect(requestTransferList({ type: "estimate", networkBandBytes: 1, totalCellBytes: 2 })).toEqual([]);
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
            { type: "done", warnings: [], summary: { cells: 1, bytes: 2, manifest: "MS1.OBS", shards: [] } },
            { type: "error", code: "capacity", message: "too big" },
        ];
        for (const msg of messages) expect(isWorkerResponse(msg)).toBe(true);
    });

    it("rejects strays instead of letting them drive the download screen", () => {
        expect(isWorkerResponse(null)).toBe(false);
        expect(isWorkerResponse({})).toBe(false);
        expect(isWorkerResponse({ type: "progress", phase: "warp", fraction: 0.1 })).toBe(false);
        expect(isWorkerResponse({ type: "error", code: "not-a-code", message: "x" })).toBe(false);
        expect(isWorkerResponse({ type: "file", name: "x", role: "core", sha256: "", byteLength: 1, bytes: [1] })).toBe(
            false,
        );
    });
});
