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

import { AssembleError, assembleCells, estimateMemory } from "./bridge";
import {
    responseTransferList,
    type AssembleWorkerRequest,
    type AssembleWorkerResponse,
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
            post({
                type: "estimate-result",
                estimate: await estimateMemory(req.networkBandBytes, req.totalCellBytes, req.budgetBytes),
            });
            return;
        }
        // Streamed shards are gone from `result.files`, so `planned` would
        // otherwise price a country-scale set at the manifest's 128 bytes.
        let streamedBytes = 0;
        const result = await assembleCells(
            req.cells,
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
            req.streamShards
                ? (file) => {
                      const byteLength = file.bytes.byteLength;
                      streamedBytes += byteLength;
                      post({ type: "shard", ...file, byteLength });
                  }
                : undefined,
        );
        try {
            post({
                type: "planned",
                totalBytes: result.files.reduce((sum, file) => sum + file.byteLength, streamedBytes),
                shardCount: result.summary.shards.length,
                warnings: [...result.warnings],
                summary: result.summary,
            });
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
