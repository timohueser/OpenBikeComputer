// The assembly worker (#1038): `bridge.ts` run where its header says it must be.
//
// This file is deliberately near-empty. The threading contract lives in the
// bridge's header, the message vocabulary in `workerProtocol.ts`, and everything
// here is the ten lines that connect them: receive a request, run the bridge,
// post what happened. The UI's cancel button never sends a message — it calls
// `worker.terminate()` on the other side, because a worker blocked inside a
// synchronous wasm call cannot read its inbox (bridge header, "Threading").
//
// Progress posts from inside the assembly's callback: the callback runs on this
// thread between wasm steps, and `postMessage` queues across without waiting.
// Files post one at a time with their buffer in the transfer list, then wait
// for the consumer's ack. The worker-side copy is gone when queued and the
// next copy stays in wasm until the browser save or SD write has finished.
//
// A `streamShards` request posts each shard the same way but from *inside* the
// assembly (#1116 B1), which is what lets wasm free it mid-run. Nothing can be
// awaited there — the run is blocked behind the callback — so those messages
// carry no ack and the consumer gets them before `planned`. The bytes are out of
// wasm memory before the post; they are on this thread only until the transfer
// queues them.
//
// A `shardSink` request goes one better (#1116 D1): the shards are written
// *into OPFS* from inside the assembly, so they are never in wasm memory and
// never cross this port. The pool of sync access handles is opened before the
// run for the same reason the input's are — the opener is async — and closed in
// the same `finally`, which is also what lets the page open the files: a sync
// handle is an exclusive lock. Each shard is then announced as a `stored-shard`
// naming its OPFS entry, and the page saves a `Blob` from it. This is not the
// same saving as `streamShards` by another route: that one still holds one whole
// shard, and the core shard cannot be split, so a country's is one ~3 GiB buffer.
//
// The input side is the mirror of that (#1116 B2). A request with `sourceCells`
// brings no cell buffers at all: the download left them in OPFS, and *this*
// thread is where they can be read back synchronously, because
// `FileSystemSyncAccessHandle` exists only in a dedicated worker. The handles are
// opened before the run (they cannot be opened during it — the opener is async
// and the run cannot await), handed to the engine as a read callback, and closed
// in a `finally`, because a handle is an exclusive lock and a leaked one would
// make the next run fail to open the same cell. A browser with OPFS but no sync
// handles reads them back into memory instead and assembles as it always did.
//
// The `finally` covers every ending except the one that skips all code: a cancel,
// which is `worker.terminate()`. A sync access handle's lock belongs to the agent
// that opened it, so terminating this one releases them — which is just as well,
// since there is no way to run anything here afterwards.

import { AssembleError, assembleCells, estimateMemory, type AssembleCell, type AssembleSources } from "./bridge";
import {
    openCellReader,
    openShardSink,
    readCellBytes,
    syncReadsAvailable,
    type CellReader,
    type ShardSink,
} from "../cells/store";
import {
    responseTransferList,
    type AssembleWorkerRequest,
    type AssembleWorkerResponse,
    type WorkerSourceCell,
    type WorkerStoredShard,
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

let acknowledge: (() => void) | null = null;

function waitForFileAck(): Promise<void> {
    return new Promise((resolve) => {
        acknowledge = resolve;
    });
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
    if (req.type === "file-ack") {
        const resolve = acknowledge;
        acknowledge = null;
        resolve?.();
        return;
    }
    try {
        if (req.type === "estimate") {
            // The input mode is a conjunction and this thread owns the second half: the main
            // thread says whether a writable store with room exists, only the worker can say
            // whether *it* can read one synchronously. Probed here so the projection prices the
            // run the assembly will actually be — a browser whose probe fails runs the buffered
            // fallback, full cells resident, and must be priced as such.
            const inputOnDisk = req.inputOnDisk && (await syncReadsAvailable());
            post({
                type: "estimate-result",
                estimate: await estimateMemory(
                    req.networkBandBytes,
                    req.totalCellBytes,
                    req.terrainBytes,
                    { inputOnDisk, streamedShardBytes: req.streamedShardBytes },
                    req.budgetBytes,
                ),
                // The same selection priced as the device path runs it: set kept until `planned`
                // (#1116 B1's opt-out). `sendToDevice` gates on this one.
                deviceEstimate: await estimateMemory(
                    req.networkBandBytes,
                    req.totalCellBytes,
                    req.terrainBytes,
                    { inputOnDisk, streamedShardBytes: 0 },
                    req.budgetBytes,
                ),
            });
            return;
        }
        // Streamed shards are gone from `result.files`, so `planned` would
        // otherwise price a country-scale set at the manifest's 128 bytes.
        let streamedBytes = 0;
        const onDisk = req.sourceCells ?? [];
        let opened: Opened = { extra: [], reader: null };
        if (onDisk.length > 0 && req.cellStore) {
            opened = await openSources(req.cellStore, onDisk);
        } else {
            post({ type: "reading", mode: "memory", cells: req.cells.length });
        }
        // Asked for and answered before the run, because the whole pool has to be
        // open before the first shard is planned. A browser that cannot serve it
        // falls back to whatever `streamShards` asked for, which is the pre-D1
        // profile and an honest path rather than a failure.
        const sink: ShardSink | null = req.shardSink ? await openShardSink() : null;
        post({ type: "writing", mode: sink ? "disk" : "memory" });
        // Announced after the run, not from inside it: the page cannot open a file
        // this worker still holds a sync handle on.
        const stored: WorkerStoredShard[] = [];
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
                // sends nothing here and the set is written without a `terrain` role.
                req.terrain ? { lattice: req.terrain, cells: req.terrainCells ?? [] } : undefined,
                // With a sink, no shard is in wasm memory to hand over in the first
                // place — the eviction seam is what a browser without one gets.
                req.streamShards && !sink
                    ? (file) => {
                          const byteLength = file.bytes.byteLength;
                          streamedBytes += byteLength;
                          post({ type: "shard", ...file, byteLength });
                      }
                    : undefined,
                opened.sources,
                sink
                    ? {
                          create: (slot, name) => sink.create(slot, name),
                          write: (slot, bytes) => sink.write(slot, bytes),
                          readAt: (slot, offset, into) => sink.readAt(slot, offset, into),
                          seal: (slot) => sink.seal(slot),
                          sealed: (shard) => {
                              streamedBytes += shard.byteLength;
                              stored.push({
                                  name: shard.name,
                                  role: shard.role,
                                  sha256: shard.sha256,
                                  byteLength: shard.byteLength,
                                  entry: sink.entry(shard.slot),
                              });
                          },
                      }
                    : undefined,
            );
        } finally {
            // The moment the run is over, whether it finished or threw: every
            // handle is an exclusive lock on a file the next run will want — and,
            // for the sink, one the *page* is about to want.
            opened.reader?.close();
            sink?.close();
        }
        try {
            post({
                type: "planned",
                totalBytes: result.files.reduce((sum, file) => sum + file.byteLength, streamedBytes),
                shardCount: result.summary.shards.length,
                warnings: [...result.warnings],
                summary: result.summary,
            });
            // The shards first, then the terrain shard and the manifest — §5.4's
            // order, and the order a resumable transfer wants. Each is acknowledged
            // like a `file`: the run is over, so waiting costs nothing and keeps the
            // page from opening several gigabyte-scale Blobs at once.
            for (const shard of stored) {
                post({ type: "stored-shard", ...shard });
                await waitForFileAck();
            }
            for (const file of result.files) {
                post({
                    type: "file",
                    name: file.name,
                    role: file.role,
                    sha256: file.sha256,
                    byteLength: file.byteLength,
                    bytes: file.take(),
                });
                // The consumer owns one file at a time. Waiting here keeps a
                // slow SD-card upload from queueing the rest of a country in
                // the page's message port while the worker runs ahead.
                await waitForFileAck();
            }
            post({ type: "done", warnings: [...result.warnings], summary: result.summary });
        } finally {
            result.release();
        }
    } catch (cause) {
        postError(cause);
    }
};
