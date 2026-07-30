// Fetching the cells of a selection: a plan whose size is known before the first
// request, and a bounded-concurrency run over it that verifies every object.
//
// v1 downloaded one file. v2 downloads hundreds of small ones — DACH is thousands
// of cells across four bands — and that changes two things and nothing else:
//
//   * **The verification does not soften.** Every cell carries `bytes` +
//     `sha256` in its band's index (`OBCC_Spec.md` §11.6) and every cell goes
//     through the same `fetchVerified` an artifact does. A corrupt cell is not a
//     visible failure later; it is a device that faults halfway up a mountain,
//     and there are now many more chances to ship one.
//   * **Progress becomes an aggregate.** The total is known up front — it is the
//     sum of the plan's `bytes`, the same summed-real-cell-bytes number the
//     ledger prices with — so progress is honest from the first byte rather than
//     a bar that discovers its length as it goes.
//
// The run is all-or-nothing: the first failure aborts the rest and rejects. A
// half-fetched cell set cannot be assembled into anything, so continuing would
// only buy a longer wait before the same error.
//
// Bytes are handed to `onCell` rather than accumulated into a return value on
// purpose. A DACH selection is gigabytes; a function that resolved to a `Map` of
// every cell's bytes would be a function that decides, on the caller's behalf, to
// hold the whole map in memory. The caller (P4b's wasm assembler, or a test)
// decides.

import { fetchVerified } from "../download";
import type { CatalogV2 } from "./manifest";
import type { CellEntry, CellIndexDocument } from "./satellites";
import type { SelectionResolution } from "./selection";

export interface CellDownloadItem {
    band: string;
    cell: CellEntry;
}

export interface CellDownloadPlan {
    /** In schema band order, then canonical cell id. The plan is the ordering
     *  authority; completion order is whatever the network does. */
    items: CellDownloadItem[];
    /** Summed `bytes` of every item — knowable before the fetch (§11.5). */
    totalBytes: number;
}

export interface CellDownloadProgress {
    completedCells: number;
    totalCells: number;
    /** Bytes received, including the partial bodies of cells still in flight. */
    receivedBytes: number;
    totalBytes: number;
}

export interface CellDownloadOptions {
    /**
     * Called with each cell's verified bytes, in completion order. May return a
     * promise, and the run waits for it before starting another cell in that
     * slot — so a slow consumer applies backpressure instead of queueing
     * gigabytes behind itself.
     */
    onCell: (item: CellDownloadItem, bytes: Uint8Array, index: number) => void | Promise<void>;
    onProgress?: (p: CellDownloadProgress) => void;
    /** How many cells are in flight at once. Small objects over one HTTP/2
     *  connection: enough to keep the pipe full, not so many that a hundred
     *  buffers coexist. */
    concurrency?: number;
    signal?: AbortSignal;
    fetchImpl?: typeof fetch;
    digest?: (bytes: Uint8Array) => Promise<ArrayBuffer>;
}

export const DEFAULT_CONCURRENCY = 6;

/** Whatever the abort carried, as something a caller can catch and print. */
function abortReason(signal: AbortSignal): unknown {
    return signal.reason ?? new Error("the cell download was aborted");
}

/**
 * The cells a resolved selection needs, in a stable order, with the total the
 * ledger already showed.
 *
 * A cell the selection names but the catalog does not publish is *not* in the
 * plan and is not an error here: a hole is legal by construction (a missing cell
 * is an empty leaf and the renderer paints backdrop there) and the ledger has
 * already reported it as coverage the rider is choosing to accept.
 */
export function planCells(
    resolution: SelectionResolution,
    catalog: CatalogV2,
    indices: ReadonlyMap<string, CellIndexDocument>,
): CellDownloadPlan {
    const items: CellDownloadItem[] = [];
    let totalBytes = 0;
    for (const band of catalog.schema.bands) {
        const index = indices.get(band.id);
        if (!index) continue;
        for (const id of resolution.cellsByBand.get(band.id) ?? []) {
            const cell = index.byId.get(id);
            if (!cell) continue;
            items.push({ band: band.id, cell });
            totalBytes += cell.bytes;
        }
    }
    return { items, totalBytes };
}

/**
 * Run a plan: fetch, verify and hand over every cell, at most `concurrency` at a
 * time. Resolves once every cell has been delivered; rejects on the first
 * failure, having aborted the rest.
 */
export async function downloadCells(
    plan: CellDownloadPlan,
    opts: CellDownloadOptions,
): Promise<{ cells: number; bytes: number }> {
    const total = plan.items.length;
    if (total === 0) return { cells: 0, bytes: 0 };

    // One controller for the whole run, chained to the caller's signal: a
    // failure in any slot cancels the others' in-flight bodies rather than
    // leaving them to finish into a run nobody is waiting for.
    const controller = new AbortController();
    const abort = () => controller.abort(opts.signal?.reason);
    if (opts.signal) {
        if (opts.signal.aborted) abort();
        else opts.signal.addEventListener("abort", abort, { once: true });
    }

    let completedCells = 0;
    let completedBytes = 0;
    const inflight = new Map<number, number>();
    const report = () => {
        let received = completedBytes;
        for (const n of inflight.values()) received += n;
        opts.onProgress?.({
            completedCells,
            totalCells: total,
            receivedBytes: received,
            totalBytes: plan.totalBytes,
        });
    };

    let cursor = 0;
    const worker = async (): Promise<void> => {
        for (;;) {
            // Checked here rather than left to the transport: an abort must stop
            // the *plan*, and a `fetch` that ignores its signal would otherwise
            // let every remaining slot start one more cell first.
            if (controller.signal.aborted) throw abortReason(controller.signal);
            const index = cursor++;
            if (index >= total) return;
            const item = plan.items[index];
            const bytes = await fetchVerified(item.cell.url, item.cell, {
                signal: controller.signal,
                fetchImpl: opts.fetchImpl,
                digest: opts.digest,
                onProgress: (p) => {
                    inflight.set(index, p.received);
                    report();
                },
            });
            inflight.delete(index);
            completedBytes += bytes.byteLength;
            completedCells += 1;
            report();
            await opts.onCell(item, bytes, index);
        }
    };

    const slots = Math.max(1, Math.min(opts.concurrency ?? DEFAULT_CONCURRENCY, total));
    try {
        // `all`, not `allSettled`: the first rejection is the answer, and the
        // abort in the catch is what stops the other slots from carrying on.
        await Promise.all(
            Array.from({ length: slots }, () =>
                worker().catch((e: unknown) => {
                    controller.abort(e);
                    throw e;
                }),
            ),
        );
    } finally {
        opts.signal?.removeEventListener("abort", abort);
    }
    return { cells: completedCells, bytes: completedBytes };
}
