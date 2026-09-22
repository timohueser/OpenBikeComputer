// The assembly worker: the bridge run where its header says it must be.
//
// This file is deliberately near-empty. The threading contract lives in the bridge's
// header, the message vocabulary in `workerProtocol.ts`, and everything here is the
// lines that connect them. The UI's cancel button never sends a message — it calls
// `worker.terminate()`, because a worker blocked inside a synchronous wasm call cannot
// read its inbox.
//
// Progress posts from inside the assembly's callback: the callback runs on this thread
// between wasm steps, and `postMessage` queues across without waiting.
//
// The output has two shapes, and the good one is the sink: with a map sink the assembly
// writes *into OPFS* from inside the blocking call, so the file is never in wasm memory
// and never crosses this port. The handle is opened before the run, because the opener
// is async, and closed in the same `finally`, which is also what lets the page read the
// file: a sync handle is an exclusive lock. A browser that cannot serve a sink buffers
// the map instead.
//
// The input side mirrors that. A request with `sourceCells` brings no cell buffers at
// all: the download left them in OPFS, and *this* thread is where they can be read back
// synchronously, because `FileSystemSyncAccessHandle` exists only in a dedicated worker.
// The handles are opened before the run and closed in a `finally`, because a leaked lock
// would make the next run fail to open the same cell.
//
// The `finally` covers every ending except the one that skips all code: a cancel, which
// is `worker.terminate()`. A sync access handle's lock belongs to the agent that opened
// it, so terminating this one releases them.

import { AssembleError, assembleCells, estimateMemory, wasmMemoryBytes, type AssembleCell, type AssembleSources, type AssembleResult } from "./bridge";
import {
    openCellReader,
    openMapSink,
    openScratchStore,
    readCellBytes,
    STORAGE_QUOTA_MESSAGE,
    syncReadsAvailable,
    takeIoStats,
    type CellReader,
    type IoStats,
    type MapSink,
    type ScratchFiles,
} from "../cells/store";
import {
    responseTransferList,
    type AssembleWorkerRequest,
    type AssembleWorkerResponse,
    type WorkerSourceCell,
} from "./workerProtocol";

function post(res: AssembleWorkerResponse): void {
    self.postMessage(res, { transfer: responseTransferList(res) });
}

function postError(cause: unknown, estimateId?: number): void {
    if (cause instanceof AssembleError) {
        post({ type: "error", code: cause.code, message: cause.message, estimateId });
    } else {
        post({
            type: "error",
            code: "internal",
            estimateId,
            message: cause instanceof Error ? cause.message : String(cause),
        });
    }
}

/**
 * Get at the cells a request left on disk, the best way this browser allows, and say
 * which way that was.
 *
 * Both outcomes are honest paths, not a success and a failure: the `buffered` one is
 * exactly today's memory profile with the download resumed.
 */
interface Opened {
    sources?: AssembleSources;
    /** Cells read back whole, for the browsers that cannot read them any other
     *  way. Empty on both other paths. */
    extra: AssembleCell[];
    reader: CellReader | null;
}

/** The store sink's four byte-moving methods, bound so the engine can call them straight.
 *  Written out rather than spread, because a spread of an object with getters would copy
 *  `open` as a snapshot. */
function sinkMethods(sink: MapSink) {
    return {
        create: () => sink.create(),
        write: (bytes: Uint8Array) => sink.write(bytes),
        readAt: (offset: number, into: Uint8Array) => sink.readAt(offset, into),
        seal: () => sink.seal(),
    };
}

async function openSources(store: string, cells: WorkerSourceCell[], requireDisk: boolean): Promise<Opened> {
    const keys = cells.map((c) => c.key);
    if (await syncReadsAvailable()) {
        const reader = await openCellReader(store, keys);
        post({ type: "reading", mode: "streamed", cells: cells.length });
        return { sources: { cells, read: (slot, offset, into) => reader.read(slot, offset, into) }, extra: [], reader };
    }
    if (requireDisk) throw new AssembleError("capacity", "The required disk-backed cell reader is no longer available.");
    const bytes = await readCellBytes(store, keys);
    post({ type: "reading", mode: "buffered", cells: cells.length });
    return { extra: cells.map((c, i) => ({ ...c, bytes: bytes[i] })), reader: null };
}

self.onmessage = async (event: MessageEvent<AssembleWorkerRequest>) => {
    const req = event.data;
    try {
        if (req.type === "estimate") {
            // Both residency escapes are conjunctions and this thread owns the second half:
            // the main thread says whether a writable store with room exists, and only the
            // worker can say whether *it* can hold sync access handles. Probed here so the
            // projection prices the run the assembly will actually be.
            const onDisk = req.onDisk && (await syncReadsAvailable());
            post({
                type: "estimate-result",
                estimateId: req.estimateId,
                onDisk,
                estimate: await estimateMemory(
                    req.networkBandBytes,
                    req.totalCellBytes,
                    req.terrainBytes,
                    req.mergeBudgetBytes,
                    { inputOnDisk: onDisk, outputSunk: onDisk },
                    req.budgetBytes,
                ),
            });
            return;
        }
        const fromDisk = req.sourceCells ?? [];
        let opened: Opened = { extra: [], reader: null };
        let sink: MapSink | null = null;
        let scratch: ScratchFiles | null = null;
        let result: AssembleResult | undefined;
        let io: IoStats | undefined;
        try {
            try {
                if (req.requireDisk && (req.cells.length > 0 || (fromDisk.length > 0 && !req.cellStore))) {
                    throw new AssembleError("capacity", "This assembly requires its downloaded cells to stay on disk.");
                }
                if (fromDisk.length > 0 && req.cellStore) {
                    opened = await openSources(req.cellStore, fromDisk, req.requireDisk);
                } else {
                    post({ type: "reading", mode: "memory", cells: req.cells.length });
                }
                sink = await openMapSink();
                if (req.requireDisk && !sink) {
                    throw new AssembleError("capacity", "The required disk-backed map output is no longer available.");
                }
                scratch = (await syncReadsAvailable()) ? await openScratchStore() : null;
                if (req.requireDisk && !scratch) {
                    throw new AssembleError("capacity", "The required disk-backed assembly scratch is no longer available.");
                }
                post({ type: "writing", mode: sink ? "disk" : "memory" });
                try {
                    result = await assembleCells(
                        [...req.cells, ...opened.extra],
                        req.schemaJson,
                        req.skinJson,
                        req.options,
                        (phase, fraction) => {
                            post({ type: "progress", phase, fraction });
                        },
                        req.knownEmpty,
                        // The raster, when the catalog publishes one. A terrain-less catalog
                        // sends nothing and the map is written with an empty terrain region.
                        req.terrain ? { lattice: req.terrain, cells: req.terrainCells ?? [] } : undefined,
                        opened.sources,
                        // Adapted rather than passed through: the store's sink is a file and
                        // knows nothing about identities. `sealed` has nothing to do here — the
                        // same digest and length arrive on the result — but the seam requires it.
                        sink ? { ...sinkMethods(sink), sealed: () => {} } : undefined,
                        scratch ?? undefined,
                    );
                } catch (cause) {
                    if (sink?.quotaExceeded || scratch?.quotaExceeded) {
                        throw new AssembleError("io", STORAGE_QUOTA_MESSAGE);
                    }
                    throw cause;
                }
            } finally {
                // The moment the run is over, whether it finished or threw: every handle is
                // an exclusive lock on a file the next run will want, and for the sink one
                // the *page* is about to want. The spill is further *deleted*, not just
                // unlocked: it means nothing outside this run and holds country-scale quota.
                opened.reader?.close();
                sink?.close();
                await scratch?.discard();
                // The run's OPFS ledger, whatever the outcome. It rides the `done` message
                // because a worker's own console does not reliably surface, and it is the
                // first number an in-tab slowness report needs.
                io = takeIoStats();
            }
            // One map, announced once. A sunk one is an identity — the page reads the bytes
            // off disk itself, now that the handle above is closed; a buffered one rides
            // across with its buffer in the transfer list.
            if (result.resident) {
                post({
                    type: "file",
                    sha256: result.sha256,
                    byteLength: result.byteLength,
                    bytes: result.take(),
                });
            } else {
                post({ type: "stored-map", sha256: result.sha256, byteLength: result.byteLength });
            }
            post({ type: "done", warnings: [...result.warnings], summary: result.summary, wasmMemoryBytes: wasmMemoryBytes(), io });
        } finally {
            result?.release();
        }
    } catch (cause) {
        postError(cause, req.type === "estimate" ? req.estimateId : undefined);
    }
};
