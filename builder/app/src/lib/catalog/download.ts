// Plans and downloads a selection with bounded concurrency, exact aggregate
// progress, and per-cell length/digest verification. Verified bytes stream to
// `onCell` so a country-scale map is never accumulated here. The first failure
// aborts the remaining requests and prevents further sink calls. Downloads are
// not persisted or resumed; that belongs in a host-level content cache.

import { fetchVerified } from "../download";
import type { Catalog } from "./manifest";
import { knownEmptyAt, type CellEntry, type CellIndexDocument } from "./satellites";
import type { SelectionResolution } from "./selection";

export interface CellDownloadItem {
    band: string;
    cell: CellEntry;
}

export interface CellDownloadPlan {
    /** In schema band order, then canonical cell id. The plan is the ordering
     *  authority; completion order is whatever the network does. */
    items: CellDownloadItem[];
    /** Selected canonical-empty cells. They have no request or byte buffer,
     *  but the assembler needs their identities for bbox and coverage math. */
    knownEmpty: { band: string; id: string }[];
    /** Summed `bytes` of every item — knowable before the fetch (§6). */
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
    catalog: Catalog,
    indices: ReadonlyMap<string, CellIndexDocument>,
): CellDownloadPlan {
    const items: CellDownloadItem[] = [];
    const knownEmpty: { band: string; id: string }[] = [];
    let totalBytes = 0;
    for (const band of catalog.schema.bands) {
        const index = indices.get(band.id);
        if (!index) continue;
        for (const id of resolution.cellsByBand.get(band.id) ?? []) {
            const cell = index.byId.get(id);
            if (cell) {
                items.push({ band: band.id, cell });
                totalBytes += cell.bytes;
            } else if (knownEmptyAt(index, id)) {
                knownEmpty.push({ band: band.id, id });
            }
        }
    }
    return { items, knownEmpty, totalBytes };
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
            let bytes: Uint8Array;
            try {
                bytes = await fetchVerified(item.cell.url, item.cell, {
                    signal: controller.signal,
                    fetchImpl: opts.fetchImpl,
                    digest: opts.digest,
                    onProgress: (p) => {
                        inflight.set(index, p.received);
                        report();
                    },
                });
            } finally {
                // `finally`, because a cell that failed or was aborted mid-body
                // still has its partial byte count in `inflight`, and every
                // other slot's progress report would keep adding it — a bar
                // that creeps past what was actually received, for bytes that
                // will never arrive.
                inflight.delete(index);
            }
            completedBytes += bytes.byteLength;
            completedCells += 1;
            report();
            // A cell that arrives after another slot has failed belongs to a run
            // that is already over. Handing it to `onCell` would write it into
            // an assembly the caller is about to throw away — and the caller's
            // sink is a file, a database, or a wasm assembler, none of which
            // enjoy a write after the rejection.
            if (controller.signal.aborted) throw abortReason(controller.signal);
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
