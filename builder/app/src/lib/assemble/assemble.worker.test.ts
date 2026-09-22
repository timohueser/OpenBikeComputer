import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AssembleWorkerRequest, AssembleWorkerResponse } from "./workerProtocol";

const seams = vi.hoisted(() => ({
    assemble: vi.fn(), estimate: vi.fn(), sync: vi.fn(), reader: vi.fn(), sink: vi.fn(), scratch: vi.fn(),
    readBytes: vi.fn(), io: vi.fn(), memory: vi.fn(),
}));
vi.mock("./bridge", async (original) => ({ ...await original<typeof import("./bridge")>(), assembleCells: seams.assemble, estimateMemory: seams.estimate, wasmMemoryBytes: seams.memory }));
vi.mock("../cells/store", () => ({
    openCellReader: seams.reader, openMapSink: seams.sink, openScratchStore: seams.scratch,
    readCellBytes: seams.readBytes, syncReadsAvailable: seams.sync, takeIoStats: seams.io,
    STORAGE_QUOTA_MESSAGE: "The browser ran out of storage while building this map. Reduce the map selection and try again.",
}));

const input = { id: "18/1204/1052", band: "network", partial: false, byteLength: 4, key: "cell" };
const request = (): Extract<AssembleWorkerRequest, { type: "assemble" }> => ({
    type: "assemble", requireDisk: true, cells: [], sourceCells: [input], cellStore: "revision",
    knownEmpty: [], schemaJson: "{}", skinJson: "{}", options: {},
});

describe("assembly worker storage admission", () => {
    let messages: AssembleWorkerResponse[];
    let dispatch: (event: MessageEvent<AssembleWorkerRequest>) => Promise<void>;
    let reader: { read: ReturnType<typeof vi.fn>; close: ReturnType<typeof vi.fn> };
    let sink: { close: ReturnType<typeof vi.fn>; quotaExceeded: boolean };
    let scratch: { discard: ReturnType<typeof vi.fn>; quotaExceeded: boolean };
    let result: { resident: boolean; sha256: string; byteLength: number; take: ReturnType<typeof vi.fn>; release: ReturnType<typeof vi.fn>; warnings: never[]; summary: object };

    beforeEach(async () => {
        vi.resetModules();
        vi.resetAllMocks();
        messages = [];
        reader = { read: vi.fn(), close: vi.fn() };
        sink = { close: vi.fn(), quotaExceeded: false };
        scratch = { discard: vi.fn(async () => undefined), quotaExceeded: false };
        result = { resident: false, sha256: "abc", byteLength: 4, take: vi.fn(() => Uint8Array.of(1, 2, 3, 4)), release: vi.fn(), warnings: [], summary: {} };
        seams.sync.mockResolvedValue(true);
        seams.reader.mockResolvedValue(reader);
        seams.sink.mockResolvedValue(sink);
        seams.scratch.mockResolvedValue(scratch);
        seams.assemble.mockResolvedValue(result);
        seams.estimate.mockResolvedValue({ fits: true });
        seams.memory.mockReturnValue(404_946_944);
        const worker = { onmessage: undefined, postMessage: (message: AssembleWorkerResponse) => messages.push(message) };
        vi.stubGlobal("self", worker);
        await import("./assemble.worker");
        dispatch = worker.onmessage!;
    });

    afterEach(() => vi.unstubAllGlobals());

    const send = (data: AssembleWorkerRequest) => dispatch({ data } as MessageEvent<AssembleWorkerRequest>);

    it.each(["input", "sink", "scratch"])("refuses lost required %s backing before assembly", async (lost) => {
        if (lost === "input") seams.sync.mockResolvedValue(false);
        if (lost === "sink") seams.sink.mockResolvedValue(null);
        if (lost === "scratch") seams.scratch.mockResolvedValue(null);
        await send(request());
        expect(messages.at(-1)).toMatchObject({ type: "error", code: "capacity" });
        expect(seams.assemble).not.toHaveBeenCalled();
        expect(seams.readBytes).not.toHaveBeenCalled();
        expect(reader.close).toHaveBeenCalledTimes(lost === "input" ? 0 : 1);
        expect(sink.close).toHaveBeenCalledTimes(lost === "scratch" ? 1 : 0);
    });

    it.each(["sink", "scratch"] as const)("closes partial acquisition when %s opening throws", async (lost) => {
        seams[lost].mockRejectedValue(new Error("storage open failed"));
        await send(request());
        expect(messages.at(-1)).toMatchObject({ type: "error", message: "storage open failed" });
        expect(seams.assemble).not.toHaveBeenCalled();
        expect(reader.close).toHaveBeenCalledOnce();
        expect(sink.close).toHaveBeenCalledTimes(lost === "scratch" ? 1 : 0);
    });

    it("rejects resident map cells even if OPFS can open", async () => {
        await send({ ...request(), cells: [{ ...input, bytes: Uint8Array.of(1) }] });
        expect(messages.at(-1)).toMatchObject({ type: "error", code: "capacity" });
        expect(seams.assemble).not.toHaveBeenCalled();
    });

    it("keeps terrain resident and permits a known-empty map selection", async () => {
        const terrain = { postingLog2: 14, cellLog2: 19 };
        const terrainCells = [{ id: "19/602/526", sha256: "terrain", bytes: Uint8Array.of(1) }];
        await send({ ...request(), sourceCells: [], cellStore: undefined, knownEmpty: [{ id: input.id, band: input.band }], terrain, terrainCells });
        expect(seams.assemble.mock.calls[0][6]).toEqual({ lattice: terrain, cells: terrainCells });
        expect(messages).toContainEqual({ type: "stored-map", sha256: "abc", byteLength: 4 });
        expect(sink.close).toHaveBeenCalledOnce();
        expect(scratch.discard).toHaveBeenCalledOnce();
        expect(result.release).toHaveBeenCalledOnce();
    });

    it("preserves an explicitly admitted buffered fallback", async () => {
        seams.sync.mockResolvedValue(false);
        seams.sink.mockResolvedValue(null);
        seams.readBytes.mockResolvedValue([Uint8Array.of(1, 2, 3, 4)]);
        result.resident = true;
        await send({ ...request(), requireDisk: false });
        expect(seams.readBytes).toHaveBeenCalledOnce();
        expect(seams.assemble.mock.calls[0][8]).toBeUndefined();
        expect(seams.assemble.mock.calls[0][9]).toBeUndefined();
        expect(messages).toContainEqual({ type: "file", sha256: "abc", byteLength: 4, bytes: Uint8Array.of(1, 2, 3, 4) });
        expect(result.release).toHaveBeenCalledOnce();
    });

    /** The seam a memory gate reads. It is taken after the run, where linear memory is at its
     *  peak, so a figure read any earlier would understate the assembly. */
    it("reports the instance's linear memory when the run is done", async () => {
        await send(request());
        expect(messages.at(-1)).toMatchObject({ type: "done", wasmMemoryBytes: 404_946_944 });
        expect(seams.memory.mock.invocationCallOrder[0]).toBeGreaterThan(seams.assemble.mock.invocationCallOrder[0]);
    });

    it("releases an assembled result when scratch cleanup rejects", async () => {
        scratch.discard.mockRejectedValue(new Error("scratch cleanup failed"));
        await send(request());
        expect(reader.close).toHaveBeenCalledOnce();
        expect(sink.close).toHaveBeenCalledOnce();
        expect(result.release).toHaveBeenCalledOnce();
        expect(messages.at(-1)).toMatchObject({ type: "error", message: "scratch cleanup failed" });
        expect(messages.some((m) => m.type === "stored-map" || m.type === "file" || m.type === "done")).toBe(false);
    });

    it("reports quota provenance without relabelling an ordinary scratch failure", async () => {
        scratch.quotaExceeded = true;
        seams.assemble.mockRejectedValueOnce(new Error("the scratch store's append returned a falsy value"));
        await send(request());
        expect(messages.at(-1)).toMatchObject({
            type: "error",
            code: "io",
            message: "The browser ran out of storage while building this map. Reduce the map selection and try again.",
        });

        scratch.quotaExceeded = false;
        seams.assemble.mockRejectedValueOnce(new Error("the scratch handle was closed"));
        await send(request());
        expect(messages.at(-1)).toMatchObject({ type: "error", message: "the scratch handle was closed" });
    });

    it("returns effective storage mode and correlates estimate failures", async () => {
        const req: AssembleWorkerRequest = { type: "estimate", estimateId: 7, onDisk: true, networkBandBytes: 1, totalCellBytes: 2, terrainBytes: 0, mergeBudgetBytes: 1024 };
        seams.sync.mockResolvedValue(false);
        await send(req);
        expect(messages.at(-1)).toMatchObject({ type: "estimate-result", estimateId: 7, onDisk: false });
        expect(seams.estimate.mock.calls[0][4]).toEqual({ inputOnDisk: false, outputSunk: false });
        seams.estimate.mockRejectedValue(new Error("estimate failed"));
        await send({ ...req, estimateId: 8 });
        expect(messages.at(-1)).toMatchObject({ type: "error", estimateId: 8, message: "estimate failed" });
    });
});
