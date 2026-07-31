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
// Files post one at a time with their buffer in the transfer list, so the
// worker-side copy of each shard is gone the moment its message is queued
// rather than when the whole set is done.

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

self.onmessage = async (event: MessageEvent<AssembleWorkerRequest>) => {
    const req = event.data;
    try {
        if (req.type === "estimate") {
            post({
                type: "estimate-result",
                estimate: await estimateMemory(req.networkBandBytes, req.totalCellBytes, req.budgetBytes),
            });
            return;
        }
        const result = await assembleCells(req.cells, req.schemaJson, req.skinJson, req.options, (phase, fraction) => {
            post({ type: "progress", phase, fraction });
        });
        try {
            for (const file of result.files) {
                post({
                    type: "file",
                    name: file.name,
                    role: file.role,
                    sha256: file.sha256,
                    byteLength: file.byteLength,
                    bytes: file.take(),
                });
            }
            post({ type: "done", warnings: [...result.warnings], summary: result.summary });
        } finally {
            result.release();
        }
    } catch (cause) {
        postError(cause);
    }
};
