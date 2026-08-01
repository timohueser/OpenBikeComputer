// The message vocabulary between the UI and the assembly worker (#1038).
//
// `bridge.ts`'s header is the contract this implements: the assembly is one
// synchronous wasm call, so it runs in a dedicated Worker; the UI's cancel is
// `worker.terminate()`; progress crosses by `postMessage`; finished files are
// transferred one at a time and acknowledged before the next. This module is
// the *words* of that conversation —
// types, guards, and transfer-list builders — kept apart from the worker's entry
// point so the protocol is testable in Node, where `Worker` does not exist.
//
// Two shapes of message discipline are load-bearing:
//
//   * **Files stream before `done`.** A volume set can be gigabytes, and holding
//     every shard on both sides of the boundary until one big result message
//     would double the set's residency at its peak. Each `file` is posted with
//     its buffer in the transfer list, so the bytes *move* rather than copy and
//     the worker's copy is gone the moment the message is queued. The worker
//     then waits for `file-ack`, so the message port cannot become a second
//     gigabyte-sized queue when the consumer is an SD card.
//   * **Every failure is one `error` message carrying the bridge's own code.**
//     The worker never rethrows into the void: an uncaught error in a worker is
//     a console line the UI cannot read. `AssembleError`'s code survives the
//     boundary because it is data here, not a class — classes do not structured-
//     clone.

import type {
    AssembleErrorCode,
    AssembleOptions,
    AssemblePhase,
    AssembleSummary,
    MemoryEstimate,
} from "./bridge";
import { ASSEMBLE_ERROR_CODES } from "./bridge";

/** One cell crossing into the worker. Same fields as `AssembleCell`, restated
 *  here because this is a wire shape: everything must structured-clone. */
export interface WorkerCell {
    id: string;
    band: string;
    partial: boolean;
    bytes: Uint8Array;
}

export interface WorkerKnownEmpty {
    id: string;
    band: string;
}

export type AssembleWorkerRequest =
    | {
          type: "estimate";
          networkBandBytes: number;
          totalCellBytes: number;
          /** Lowered by a caller that knows it is on a phone (bridge docs). */
          budgetBytes?: number;
      }
    | {
          type: "assemble";
          cells: WorkerCell[];
          knownEmpty: WorkerKnownEmpty[];
          /** The catalog root body, verbatim — the engine reads the schema out
           *  of it (`Schema::parse` accepts an OBCC v2 root). */
          schemaJson: string;
          /** The chosen skin entry, as JSON. */
          skinJson: string;
          options: AssembleOptions;
      }
    | { type: "file-ack" };

/** One finished file, bytes transferred. Mirrors `AssembledFile` minus `take()`,
 *  which has already happened on the worker side. */
export interface WorkerFile {
    name: string;
    role: "core" | "coarse" | "geometry" | "manifest";
    sha256: string;
    byteLength: number;
    bytes: Uint8Array;
}

export type AssembleWorkerResponse =
    | { type: "progress"; phase: AssemblePhase; fraction: number }
    | { type: "planned"; totalBytes: number; shardCount: number; warnings: string[]; summary: AssembleSummary }
    | ({ type: "file" } & WorkerFile)
    | { type: "done"; warnings: string[]; summary: AssembleSummary }
    | { type: "estimate-result"; estimate: MemoryEstimate }
    | { type: "error"; code: AssembleErrorCode; message: string };

/** The transfer list for an `assemble` request: every cell's buffer moves into
 *  the worker rather than copying — the main thread has no further use for
 *  gigabytes of downloaded cells once the assembly owns them. */
export function requestTransferList(req: AssembleWorkerRequest): Transferable[] {
    if (req.type !== "assemble") return [];
    return dedupedBuffers(req.cells.map((c) => c.bytes));
}

/** The transfer list for a `file` response. */
export function responseTransferList(res: AssembleWorkerResponse): Transferable[] {
    if (res.type !== "file") return [];
    return dedupedBuffers([res.bytes]);
}

/** Each distinct ArrayBuffer once — transferring the same buffer twice throws,
 *  and two views can share one (a test fixture does; a future pooled download
 *  could). */
function dedupedBuffers(views: Uint8Array[]): ArrayBuffer[] {
    const seen = new Set<ArrayBuffer>();
    for (const v of views) {
        if (v.buffer instanceof ArrayBuffer) seen.add(v.buffer);
    }
    return [...seen];
}

const PHASES: ReadonlySet<string> = new Set([
    "open",
    "poi",
    "nav",
    "plan",
    "write",
    "verify",
    "manifest",
    "done",
]);
const ROLES: ReadonlySet<string> = new Set(["core", "coarse", "geometry", "manifest"]);
const CODES: ReadonlySet<string> = new Set(ASSEMBLE_ERROR_CODES);

/**
 * Whether a value that arrived over `onmessage` is a response this protocol
 * speaks. A worker's inbox is an untyped seam — a stray message (a devtools
 * extension, a future protocol revision) must be droppable rather than crash the
 * download screen — so the guard checks shape, not just the tag.
 */
export function isWorkerResponse(v: unknown): v is AssembleWorkerResponse {
    if (typeof v !== "object" || v === null) return false;
    const m = v as Record<string, unknown>;
    switch (m.type) {
        case "progress":
            return typeof m.phase === "string" && PHASES.has(m.phase) && typeof m.fraction === "number";
        case "planned":
            return (
                Number.isSafeInteger(m.totalBytes) &&
                (m.totalBytes as number) >= 0 &&
                Number.isInteger(m.shardCount) &&
                (m.shardCount as number) >= 1 &&
                Array.isArray(m.warnings) &&
                typeof m.summary === "object" &&
                m.summary !== null
            );
        case "file":
            return (
                typeof m.name === "string" &&
                typeof m.role === "string" &&
                ROLES.has(m.role) &&
                typeof m.sha256 === "string" &&
                typeof m.byteLength === "number" &&
                m.bytes instanceof Uint8Array
            );
        case "done":
            return Array.isArray(m.warnings) && typeof m.summary === "object" && m.summary !== null;
        case "estimate-result":
            return typeof m.estimate === "object" && m.estimate !== null;
        case "error":
            return typeof m.code === "string" && CODES.has(m.code) && typeof m.message === "string";
        default:
            return false;
    }
}
