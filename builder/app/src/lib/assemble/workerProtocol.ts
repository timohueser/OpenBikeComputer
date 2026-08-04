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
//   * **`shard` is the same idea one step earlier, and it has no ack** (#1116
//     B1). A request with `streamShards` makes the wasm side hand each shard
//     over the moment its §4.8 verify passes, which frees it from wasm memory
//     mid-run instead of at the end — the assembly's output residency becomes
//     one shard. The catch is where the callback runs: *inside* the synchronous
//     assembly, on this thread, with no way to await anything. So a `shard`
//     posts and the run carries straight on, and these messages arrive **before
//     `planned`**, unlike every `file`. A consumer that needs the set plan first
//     (the device upload does) must not ask for them.
//   * **Cells can travel as names instead of bytes** (#1116 B2). `sourceCells`
//     carries an identity, a length and an OPFS filename per cell, and *nothing*
//     is transferred: the download already put them on disk, and the worker is
//     the only thread that can read them synchronously — which is what the wasm
//     engine needs, since it reads them from inside one blocking call. A request
//     with `cells` is the same conversation with the bytes in it, for a browser
//     with no usable storage.
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

/** One cell that stayed on disk (#1116 B2). Same identity as {@link WorkerCell},
 *  with the catalog's byte count and the OPFS filename — its digest — instead of
 *  the bytes. Nothing is transferred: that is the point. */
export interface WorkerSourceCell {
    id: string;
    band: string;
    partial: boolean;
    byteLength: number;
    key: string;
}

export interface WorkerKnownEmpty {
    id: string;
    band: string;
}

/** One terrain cell crossing into the worker. Same fields as
 *  `AssembleTerrainCell`; a canonically void square is simply not sent, because
 *  it has no object at all (`OBCC_Spec.md` §13.6). */
export interface WorkerTerrainCell {
    id: string;
    sha256: string;
    bytes: Uint8Array;
}

/** The catalog's terrain lattice (`OBCC_Spec.md` §13.1). Absent means the
 *  catalog publishes no raster and the set carries no `terrain` role. */
export interface WorkerTerrain {
    postingLog2: number;
    cellLog2: number;
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
          /** The cells the download left in OPFS instead of in memory (#1116
           *  B2), with `cellStore` naming the revision directory they are in.
           *  A request may carry both lists; the browser sends one or the
           *  other. */
          sourceCells?: WorkerSourceCell[];
          cellStore?: string;
          knownEmpty: WorkerKnownEmpty[];
          /** The catalog root body, verbatim — the engine reads the schema out
           *  of it (`Schema::parse` accepts an OBCC v2 root). */
          schemaJson: string;
          /** The chosen skin entry, as JSON. */
          skinJson: string;
          options: AssembleOptions;
          /** The raster (EL4). Absent for a terrain-less catalog, in which case
           *  the set is written without a `terrain` role. */
          terrain?: WorkerTerrain;
          terrainCells?: WorkerTerrainCell[];
          /** Hand each shard back as `shard` the moment it is verified, instead
           *  of holding the whole set in wasm memory until the run ends (#1116
           *  B1). Off by default, and it must stay off for a consumer that
           *  cannot take a file before `planned` — see this module's header. */
          streamShards?: boolean;
      }
    | { type: "file-ack" };

/** One finished file, bytes transferred. Mirrors `AssembledFile` minus `take()`,
 *  which has already happened on the worker side. */
export interface WorkerFile {
    name: string;
    role: "core" | "coarse" | "geometry" | "terrain" | "manifest";
    sha256: string;
    byteLength: number;
    bytes: Uint8Array;
}

/** One shard, evicted from wasm memory the moment §4.8 passed on it. Same
 *  fields as a `file`; a different tag because it arrives **mid-run, before
 *  `planned`, and is never acknowledged** (module header). */
export interface WorkerShard extends WorkerFile {
    role: "core" | "coarse" | "geometry";
}

/**
 * How the run that is starting gets at its cells (#1116 B2). Posted **before**
 * the assembly, not with its result, because the case where it matters most is
 * the one that never produces a result: a bug report about a failed country-scale
 * run has to say which path was taken.
 *
 * - `streamed` — from OPFS through sync access handles, the wasm engine reading a
 *   block at a time. Input residency is the read cache, ~1 MiB.
 * - `buffered` — from OPFS, but read back into memory first, because this browser
 *   has no sync access handles. The download still resumed from disk; the memory
 *   is what it always was.
 * - `memory` — the cells arrived over the wire in this request. No store.
 */
export type CellReadMode = "streamed" | "buffered" | "memory";

export type AssembleWorkerResponse =
    | { type: "progress"; phase: AssemblePhase; fraction: number }
    | { type: "reading"; mode: CellReadMode; cells: number }
    | { type: "planned"; totalBytes: number; shardCount: number; warnings: string[]; summary: AssembleSummary }
    | ({ type: "shard" } & WorkerShard)
    | ({ type: "file" } & WorkerFile)
    | { type: "done"; warnings: string[]; summary: AssembleSummary }
    | { type: "estimate-result"; estimate: MemoryEstimate }
    | { type: "error"; code: AssembleErrorCode; message: string };

/** The transfer list for an `assemble` request: every cell's buffer moves into
 *  the worker rather than copying — the main thread has no further use for
 *  gigabytes of downloaded cells once the assembly owns them. */
export function requestTransferList(req: AssembleWorkerRequest): Transferable[] {
    if (req.type !== "assemble") return [];
    return dedupedBuffers([...req.cells.map((c) => c.bytes), ...(req.terrainCells ?? []).map((c) => c.bytes)]);
}

/** The transfer list for a `file` or `shard` response: the bytes *move*, so the
 *  worker's copy is gone the moment the message is queued. For a `shard` that
 *  matters twice over — it was evicted from wasm memory to keep the peak down,
 *  and copying it here would put it straight back. */
export function responseTransferList(res: AssembleWorkerResponse): Transferable[] {
    if (res.type !== "file" && res.type !== "shard") return [];
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
const READ_MODES: ReadonlySet<string> = new Set(["streamed", "buffered", "memory"]);
const SHARD_ROLES: ReadonlySet<string> = new Set(["core", "coarse", "geometry"]);
const ROLES: ReadonlySet<string> = new Set([...SHARD_ROLES, "terrain", "manifest"]);
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
        case "reading":
            return typeof m.mode === "string" && READ_MODES.has(m.mode) && Number.isInteger(m.cells);
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
        case "shard":
        case "file":
            return (
                typeof m.name === "string" &&
                typeof m.role === "string" &&
                // A streamed one is always an OBCM shard: the terrain shard and
                // the manifest are not evicted, they arrive as `file`s.
                (m.type === "shard" ? SHARD_ROLES : ROLES).has(m.role) &&
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
