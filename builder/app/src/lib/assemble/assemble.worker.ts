// The assembly worker (#1038): `bridge.ts` run where its header says it must be.
//
// This file is deliberately near-empty. The threading contract lives in the
// bridge's header, the message vocabulary in `workerProtocol.ts`, and everything
// here is the lines that connect them: receive a request, run the bridge, post what
// happened. The UI's cancel button never sends a message — it calls
// `worker.terminate()` on the other side, because a worker blocked inside a
// synchronous wasm call cannot read its inbox (bridge header, "Threading").
//
// Progress posts from inside the assembly's callback: the callback runs on this
// thread between wasm steps, and `postMessage` queues across without waiting.
//
// The output has two shapes, and the good one is the sink: with a map sink
// the assembly writes *into OPFS* from inside the blocking call (#1116 D1), so the
// file is never in wasm memory and never crosses this port. The handle is opened
// before the run for the same reason the input's are — the opener is async — and
// closed in the same `finally`, which is also what lets the page read the file: a
// sync handle is an exclusive lock. The map is then announced as a `stored-map`
// carrying only its identity, and the page opens a `Blob` on the known entry. A
// browser that cannot serve a sink buffers the map instead and gets it as a `file`
// with the bytes transferred — honest, and the reason a country-scale selection is
// refused there rather than attempted.
//
// The input side is the mirror of that (#1116 B2). A request with `sourceCells`
// brings no cell buffers at all: the download left them in OPFS, and *this* thread
// is where they can be read back synchronously, because `FileSystemSyncAccessHandle`
// exists only in a dedicated worker. The handles are opened before the run (they
// cannot be opened during it — the opener is async and the run cannot await), handed
// to the engine as a read callback, and closed in a `finally`, because a handle is
// an exclusive lock and a leaked one would make the next run fail to open the same
// cell. A browser with OPFS but no sync handles reads them back into memory instead
// and assembles as it always did.
//
// The `finally` covers every ending except the one that skips all code: a cancel,
// which is `worker.terminate()`. A sync access handle's lock belongs to the agent
// that opened it, so terminating this one releases them — which is just as well,
// since there is no way to run anything here afterwards.

import { AssembleError, assembleCells, estimateMemory, type AssembleCell, type AssembleSources } from "./bridge";
import {
    openCellReader,
    openMapSink,
    openScratchStore,
    readCellBytes,
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

function postError(cause: unknown): void {
    if (cause instanceof AssembleError) {
        post({ type: "error", code: cause.code, message: cause.message });
    } else {
        post({
            type: "error",
            code: "internal",
            message: cause instanceof Error ? cause.message : String(cause),
        });
    }
}

/**
 * Get at the cells a request left on disk, the best way this browser allows, and
 * say which way that was.
 *
 * Both outcomes are honest paths, not a success and a failure: the `buffered` one
 * is exactly today's memory profile with the download resumed, which is what a
 * browser without sync access handles can have.
 */
interface Opened {
    sources?: AssembleSources;
    /** Cells read back whole, for the browsers that cannot read them any other
     *  way. Empty on both other paths. */
    extra: AssembleCell[];
    reader: CellReader | null;
}

/** The store sink's four byte-moving methods, bound so the engine can call them
 *  straight. Written out rather than spread, because a spread of an object with
 *  getters would copy `open` as a snapshot. */
function sinkMethods(sink: MapSink) {
    return {
        create: () => sink.create(),
        write: (bytes: Uint8Array) => sink.write(bytes),
        readAt: (offset: number, into: Uint8Array) => sink.readAt(offset, into),
        seal: () => sink.seal(),
    };
}

async function openSources(store: string, cells: WorkerSourceCell[]): Promise<Opened> {
    const keys = cells.map((c) => c.key);
    if (await syncReadsAvailable()) {
        const reader = await openCellReader(store, keys);
        post({ type: "reading", mode: "streamed", cells: cells.length });
        return { sources: { cells, read: (slot, offset, into) => reader.read(slot, offset, into) }, extra: [], reader };
    }
    const bytes = await readCellBytes(store, keys);
    post({ type: "reading", mode: "buffered", cells: cells.length });
    return { extra: cells.map((c, i) => ({ ...c, bytes: bytes[i] })), reader: null };
}

self.onmessage = async (event: MessageEvent<AssembleWorkerRequest>) => {
    const req = event.data;
    try {
        if (req.type === "estimate") {
            // Both residency escapes are conjunctions and this thread owns the second half: the
            // main thread says whether a writable store with room exists, only the worker can say
            // whether *it* can hold sync access handles. Probed here so the projection prices the
            // run the assembly will actually be — a browser whose probe fails reads full cells into
            // memory and buffers the finished map there too, and must be priced as such.
            const onDisk = req.onDisk && (await syncReadsAvailable());
            post({
                type: "estimate-result",
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
        // Named apart from the estimate branch's `onDisk` boolean on purpose: this is the list of
        // cells the download left in OPFS, not a verdict about whether it could.
        const fromDisk = req.sourceCells ?? [];
        let opened: Opened = { extra: [], reader: null };
        if (fromDisk.length > 0 && req.cellStore) {
            opened = await openSources(req.cellStore, fromDisk);
        } else {
            post({ type: "reading", mode: "memory", cells: req.cells.length });
        }
        // Asked for and answered before the run, because the handle cannot be
        // opened once the blocking call has started. A browser that cannot serve it
        // buffers the map in wasm memory instead — an honest path rather than a
        // failure, and the one the memory projection already refused a country on.
        const sink: MapSink | null = await openMapSink();
        post({ type: "writing", mode: sink ? "disk" : "memory" });
        // The engine's spill (#1116 D2) goes to OPFS whenever this worker can hold
        // sync handles at all — it is worth wiring even when the cells arrived in
        // memory, because from D3 on the spill is the merge's own edge stream.
        // `null` falls back to spilling inside wasm, which is correct and priced
        // honestly, just not the point.
        const scratch: ScratchFiles | null = (await syncReadsAvailable()) ? await openScratchStore() : null;
        let io: IoStats | undefined;
        let result;
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
                // sends nothing here and the map is written with an empty §1.3 region.
                req.terrain ? { lattice: req.terrain, cells: req.terrainCells ?? [] } : undefined,
                opened.sources,
                // Adapted rather than passed through: the store's sink is a file and
                // knows nothing about identities. `sealed` has genuinely nothing to
                // do here — the same digest and length arrive on the result, from the
                // same place — but the seam requires it, and a sink that could not
                // report a finished file would be one whose bytes nobody can name.
                sink ? { ...sinkMethods(sink), sealed: () => {} } : undefined,
                scratch ?? undefined,
            );
        } finally {
            // The moment the run is over, whether it finished or threw: every
            // handle is an exclusive lock on a file the next run will want — and,
            // for the sink, one the *page* is about to want. The spill is further
            // *deleted*, not just unlocked: it means nothing outside this run and
            // holds country-scale quota.
            opened.reader?.close();
            sink?.close();
            await scratch?.discard();
            // The run's OPFS ledger, whatever the outcome: every crossing into
            // the browser's storage, by channel, with its wall-clock cost. It
            // rides the `done` message because a worker's own console does not
            // reliably surface — and it is the first number an in-tab slowness
            // report needs.
            io = takeIoStats();
        }
        try {
            // One map, announced once. A sunk one is an identity — the page reads the
            // bytes off disk itself, now that the handle above is closed; a buffered
            // one rides across with its buffer in the transfer list.
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
            post({ type: "done", warnings: [...result.warnings], summary: result.summary, io });
        } finally {
            result.release();
        }
    } catch (cause) {
        postError(cause);
    }
};
