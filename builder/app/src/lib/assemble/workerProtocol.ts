// The message vocabulary between the UI and the assembly worker (#1038).
//
// `bridge.ts`'s header is the contract this implements: the assembly is one
// synchronous wasm call, so it runs in a dedicated Worker; the UI's cancel is
// `worker.terminate()`; progress crosses by `postMessage`. This module is the
// *words* of that conversation — types, guards, and transfer-list builders — kept
// apart from the worker's entry point so the protocol is testable in Node, where
// `Worker` does not exist.
//
// **A run produces exactly one map, and says so exactly once.** Which of the two
// ways it says it is the whole shape of this protocol:
//
//   * **`stored-map`** — the assembly wrote the file straight into OPFS through a
//     sync access handle (#1116 D1), so it was never in wasm memory and never
//     crossed this port. What arrives is an identity: a digest and a length. The
//     worker posts it only after closing its handle, because a sync access handle
//     is an exclusive lock and the page cannot open the file while the worker holds
//     one. The page then opens a `Blob` on it and saves it. This is the path that
//     matters: a DACH map is a single ~9 GiB object, larger than the wasm32 address
//     space it would otherwise have to fit in.
//   * **`file`** — the browser could not serve a sink, so the map was buffered and
//     its bytes ride the port with the buffer in the transfer list. They *move*
//     rather than copy, so the worker's copy is gone the moment the message is
//     queued.
//
// There is no acknowledgement in either direction and nothing to order: with one
// file there is no next file to hold back, so the handshake the shard era needed is
// simply gone.
//
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
import type { IoStats } from "../cells/store";

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
          /** The terrain squares' share of the total — resident whatever the
           *  mode, because they are never stored in OPFS (#1116 B2). */
          terrainBytes: number;
          /** The main thread's half of both residency escapes: a writable cell
           *  store with room for the whole run — cells, map and spill. The worker
           *  ANDs it with its own sync-read probe, which is the half only the
           *  worker can assert. */
          onDisk: boolean;
          /** The engine's sort budget (`mergeBudgetBytes` in the options), which
           *  after phase D **is** the engine term on an OPFS host. */
          mergeBudgetBytes: number;
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
           *  the map is written with an empty §1.3 region. */
          terrain?: WorkerTerrain;
          terrainCells?: WorkerTerrainCell[];
      };

/** The assembled map, bytes transferred, for a run that had to buffer it. */
export interface WorkerFile {
    sha256: string;
    byteLength: number;
    bytes: Uint8Array;
}

/** The assembled map, written **into OPFS** (#1116 D1): its identity, and nothing
 *  else. The page reads it back with `readMapOutput()` and saves it under a name it
 *  chose itself; the entry it lives in is a constant both sides already know, so
 *  nothing map-sized — and no path-shaped string — crosses this port. */
export interface WorkerStoredMap {
    sha256: string;
    byteLength: number;
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

/**
 * …and where this run's map went (#1116 D1), posted for the same reason: the run
 * that matters most is the one that fails, and its bug report has to say which path
 * it took.
 *
 * - `disk` — straight into OPFS through a sync access handle. The map is never in
 *   wasm memory, which at country scale is the only way it exists at all.
 * - `memory` — buffered in wasm memory and handed over at the end. What a browser
 *   without usable storage gets, and what binds the size of map it can build.
 */
export type MapWriteMode = "disk" | "memory";

export type AssembleWorkerResponse =
    | { type: "progress"; phase: AssemblePhase; fraction: number }
    | { type: "reading"; mode: CellReadMode; cells: number }
    | { type: "writing"; mode: MapWriteMode }
    | ({ type: "stored-map" } & WorkerStoredMap)
    | ({ type: "file" } & WorkerFile)
    | { type: "done"; warnings: string[]; summary: AssembleSummary; io?: IoStats }
    | { type: "estimate-result"; estimate: MemoryEstimate }
    | { type: "error"; code: AssembleErrorCode; message: string };

/** The transfer list for an `assemble` request: every cell's buffer moves into
 *  the worker rather than copying — the main thread has no further use for
 *  gigabytes of downloaded cells once the assembly owns them. */
export function requestTransferList(req: AssembleWorkerRequest): Transferable[] {
    if (req.type !== "assemble") return [];
    return dedupedBuffers([...req.cells.map((c) => c.bytes), ...(req.terrainCells ?? []).map((c) => c.bytes)]);
}

/** The transfer list for a `file` response: the bytes *move*, so the worker's copy
 *  is gone the moment the message is queued. Nothing else in the protocol carries
 *  bytes — a sunk map is announced as an identity and read off disk by the page. */
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

const PHASES: ReadonlySet<string> = new Set(["open", "poi", "nav", "plan", "write", "verify", "done"]);
const READ_MODES: ReadonlySet<string> = new Set(["streamed", "buffered", "memory"]);
const WRITE_MODES: ReadonlySet<string> = new Set(["disk", "memory"]);
const CODES: ReadonlySet<string> = new Set(ASSEMBLE_ERROR_CODES);

/** A map's identity, as both output messages carry it. */
function isMapIdentity(m: Record<string, unknown>): boolean {
    return (
        typeof m.sha256 === "string" &&
        m.sha256.length > 0 &&
        Number.isSafeInteger(m.byteLength) &&
        (m.byteLength as number) >= 0
    );
}

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
        case "writing":
            return typeof m.mode === "string" && WRITE_MODES.has(m.mode);
        case "stored-map":
            return isMapIdentity(m);
        case "file":
            return isMapIdentity(m) && m.bytes instanceof Uint8Array;
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
