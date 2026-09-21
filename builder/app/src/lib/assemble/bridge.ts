/**
 * Lazy wasm bridge to the shared Rust assembler. Assembly is one synchronous wasm call and must
 * run in a Worker; UI cancellation terminates that worker. All failures use {@link AssembleError}.
 *
 * The engine writes one `.obcm` and names nothing. What crosses this seam is a digest, a length
 * and, when the run buffered it, the bytes. The caller decides what the file is called.
 */

import type { InitInput } from "./pkg/obc_web_assemble.js";

/** The runtime half of {@link AssembleErrorCode}, so the type and the boundary guard cannot drift apart. */
export const ASSEMBLE_ERROR_CODES = ["input", "format", "capacity", "verify", "aborted", "io", "internal"] as const;

/**
 * Why an assembly failed. Mirrors `ErrorCode::as_str` in `apps/obc-web-assemble/src/driver.rs`;
 * add or rename in both.
 *
 * - `input` — the selection is wrong: mixed schemas, an unaccepted hole or partial cell.
 * - `format` — a cell does not honour the format; the download is corrupt.
 * - `capacity` — a per-file ceiling. Coverage must be reduced; the engine never drops cells.
 * - `verify` — the read-back rejected the output. A defect, never a retry.
 * - `aborted` — the caller's progress callback asked to stop.
 */
export type AssembleErrorCode = (typeof ASSEMBLE_ERROR_CODES)[number];

/** An assembly failure: a stable {@link AssembleErrorCode} plus the engine's own message. */
export class AssembleError extends Error {
    readonly code: AssembleErrorCode;

    constructor(code: AssembleErrorCode, message: string) {
        super(message);
        this.name = "AssembleError";
        this.code = code;
    }
}

/**
 * Which stage of the assembly is running. Mirrors `Phase::as_str` in
 * `apps/obc-web-assemble/src/driver.rs`. `nav`, `write` and `verify` are the long phases.
 */
export type AssemblePhase = "open" | "poi" | "nav" | "plan" | "write" | "verify" | "done";

/**
 * Progress sink. `fraction` is overall completion (0..1), not progress within `phase`. Return
 * `true` to ask for an abort; it is honoured at the next write, so a request during the long `nav`
 * phase takes effect when that phase ends.
 */
export type AssembleProgress = (phase: AssemblePhase, fraction: number) => boolean | void;

/** One cell, as the catalog names it and the download delivered it. */
export interface AssembleCell {
    /** The canonical cell id, `<log2>/<i>/<j>`. */
    readonly id: string;
    /** The catalog's band id; not inferable from the bytes. */
    readonly band: string;
    readonly partial?: boolean;
    readonly bytes: Uint8Array;
}

/**
 * One cell the caller keeps outside wasm memory and serves on demand — in the browser, a file in
 * OPFS read through a `FileSystemSyncAccessHandle`.
 *
 * Reads name a cell by its slot: its index in the `cells` array passed to {@link AssembleSources}.
 * `key` is the caller's own name for the bytes and is never interpreted here.
 */
export interface AssembleSourceCell {
    /** The canonical cell id, `<log2>/<i>/<j>`. */
    readonly id: string;
    /** The catalog's band id; not inferable from the bytes. */
    readonly band: string;
    readonly partial?: boolean;
    /** The catalog's byte count, which becomes the cell's length as the engine sees it. */
    readonly byteLength: number;
    /** Whatever the caller resolves reads against — the digest, for the OPFS store. */
    readonly key: string;
}

/**
 * Fill `into` with `into.byteLength` bytes at `offset` of the cell in `slot`. Return `true` on
 * success; anything else fails the assembly as `io`. A short read is a failure.
 *
 * It runs inside the synchronous assembly, so it must be synchronous itself.
 * `FileSystemSyncAccessHandle.read()` is the one browser file read that can be made from there, and
 * it exists only in a dedicated worker.
 *
 * `into` is a view onto wasm's linear memory, valid only for the duration of the call.
 */
export type AssembleRead = (slot: number, offset: number, into: Uint8Array) => boolean;

/** The cells that are not in memory, and how to read them. Both or neither. */
export interface AssembleSources {
    readonly cells: readonly AssembleSourceCell[];
    readonly read: AssembleRead;
}

/** A selected cell with canonical empty content and therefore no OBCM bytes. */
export interface AssembleKnownEmpty {
    readonly id: string;
    readonly band: string;
}

/**
 * The terrain store's lattice, verbatim from the catalog's `terrain` block. Passing it is what
 * gives the map a terrain region at all; a catalog with no terrain block passes none.
 */
export interface AssembleTerrain {
    readonly postingLog2: number;
    readonly cellLog2: number;
}

/**
 * One downloaded terrain cell: its id on the terrain grid, the digest the pinned terrain index
 * published, and the whole `.obcd` object.
 *
 * A canonically void square (open ocean, outside coverage) is not passed at all. An absent cell
 * reads the same as an all-`NODATA` one, so terrain needs no {@link AssembleKnownEmpty}.
 */
export interface AssembleTerrainCell {
    readonly id: string;
    /** Lowercase-hex SHA-256 from the terrain index. The engine re-checks it before the block is
     *  copied: nothing self-made reaches a device unverified. */
    readonly sha256: string;
    readonly bytes: Uint8Array;
}

/** What an assembly can be told to do differently. Every field optional. */
export interface AssembleOptions {
    /** Proceed although a selected cell is missing. */
    readonly acceptHoles?: boolean;
    readonly acceptPartial?: boolean;
    /** How much of a source cell one {@link AssembleRead} brings back (default 4 KiB, clamped to
     *  4 MiB). The cache holds sixteen of these. `1` turns the input cache off; only a measurement
     *  should ask for that. */
    readonly readBlockBytes?: number;
    /** The most memory the nav merge's sorted passes may hold (default 64 MiB, floor 64 KiB). It
     *  bounds what the merge holds, never what it writes: the same selection assembles to the same
     *  bytes at any budget. It only lowers residency with an {@link AssembleScratchStore} wired. */
    readonly mergeBudgetBytes?: number;
}

/** The finished map as the {@link AssembleMapSink} is told about it: an identity with no bytes. */
export interface AssembleSealedMap {
    /** Lowercase-hex SHA-256 of the whole file, the spliced raster included. */
    readonly sha256: string;
    /** How many bytes the sink was handed — what its file must be long. */
    readonly byteLength: number;
}

/**
 * Where the map itself goes, when the caller would rather wasm memory did not hold it. A DACH map
 * is a single ~9 GiB object, larger than the whole 4 GiB wasm32 address space, so at that scale a
 * sink is the only shape in which the selection exists. In the browser it is one OPFS
 * `FileSystemSyncAccessHandle`, opened in the assembly worker before the run.
 *
 * Every method runs inside the synchronous assembly, so all of them are synchronous, and:
 *
 * - Return `true`. Anything falsy, or a throw, fails the run as `io`. A short write or read is a
 *   failure, not a partial success.
 * - `bytes` and `into` are views onto wasm's linear memory, valid only for the duration of the
 *   call. Do not keep them and do not call back into the assembler.
 * - `seal` must flush: the next thing the engine does is read the map back.
 * - A failed or cancelled run can leave most of a file behind. Cleaning it up is the caller's job.
 *
 * Passing a sink makes {@link AssembleResult.resident} `false` and {@link AssembleResult.take} throw.
 */
export interface AssembleMapSink {
    /** Begin the map. A sink reusing a file must truncate it here. */
    create(): boolean;
    write(bytes: Uint8Array): boolean;
    /** Fill `into` with `into.byteLength` bytes at `offset` of the sealed map, for the read-back. */
    readAt(offset: number, into: Uint8Array): boolean;
    seal(): boolean;
    /** The map has passed verification — here is what you have. Throwing fails the run as `io`. */
    sealed(map: AssembleSealedMap): void;
}

/**
 * Where the engine's spill goes instead of into wasm memory: the sorted passes' working files, not
 * the map's input or output. In the browser this is `openScratchStore()`'s pool of OPFS sync access
 * handles. Every method runs inside the blocking assembly, so all of them are synchronous and none
 * may throw. Without one the engine spills into linear memory, byte-identical but resident.
 */
export interface AssembleScratchStore {
    /** Mint a spill file: a non-negative id, or `-1` to refuse (pool exhausted). */
    create(): number;
    /** Append to `id`. A short write is a failure. */
    append(id: number, bytes: Uint8Array): boolean;
    /** Fill `into` with exactly `into.byteLength` bytes at `offset`. A short read is a failure. */
    readAt(id: number, offset: number, into: Uint8Array): boolean;
    /** Bytes appended to `id`, or `-1` for an unknown/removed id. */
    len(id: number): number;
    /** Drop `id`. Ids are never reused; a later use of one must refuse. */
    remove(id: number): boolean;
}

/** What an assembly produced: one map, plus what the engine wants said about it. */
export interface AssembleResult {
    /** Lowercase-hex SHA-256 of the whole file — the map's identity, sink or not. */
    readonly sha256: string;
    /** The file's length. Still true after {@link AssembleResult.take} has emptied the buffer. */
    readonly byteLength: number;
    /** Whether the bytes are here to {@link AssembleResult.take}. `false` after a run with a sink. */
    readonly resident: boolean;
    /**
     * Move the map's bytes to JS and free the wasm copy. Once. A second call throws `internal`
     * rather than hand back an empty array, because the natural retry shape would write a 0-byte
     * file to a card and call it a map. Throws for a sunk run, and after
     * {@link AssembleResult.release}.
     */
    take(): Uint8Array;
    /** What the format says a producer should report rather than refuse. */
    readonly warnings: readonly string[];
    /** The engine's summary, in the shape `obcm-assemble --json` prints. */
    readonly summary: AssembleSummary;
    /**
     * Free whatever is still held in wasm memory. Call this when you are done: a map can be
     * gigabytes, and wasm-bindgen objects are not collected with their JS handles. Idempotent;
     * `take()` after it throws. A `FinalizationRegistry` is the net under a forgotten call, but
     * until the collector runs the map is still resident.
     */
    release(): void;
}

/** The summary document, as far as callers rely on it. Additional fields exist; see the CLI. */
export interface AssembleSummary {
    readonly cells: number;
    readonly bytes: number;
    readonly sha256: string;
    readonly verified: { chunks: number; features: number; nav_nodes: number } | null;
    readonly [key: string]: unknown;
}

/**
 * Projected peak wasm memory for a selection — the answer to "can this be assembled in a tab at
 * all", available before the download. What binds in a tab is the run against wasm32's 4 GiB
 * address space; the model and its constants live in `apps/obc-web-assemble/src/estimate.rs`.
 *
 * How much to trust these numbers: the engine term is a linear fit through two measured runs, and
 * {@link MemoryEstimate.budgetBytes} is a judgement rather than a measurement — browsers do not
 * publish what they will grant, and an allocation wasm cannot serve aborts the module with no error
 * to render. A comfortable `fits: false` is reliable and is the case this exists for. A `fits: true`
 * with little headroom means "probably": present it as a warning with the number, never as a
 * guarantee.
 *
 * The {@link Residency} passed in states whether the cells stream from OPFS and the map goes to a
 * sink. A browser with no usable OPFS holds both in linear memory and binds far earlier, so the two
 * verdicts genuinely differ.
 */
export interface MemoryEstimate {
    /** The engine's working set, dominated by the nav rewrite. */
    readonly engineBytes: number;
    /** The resident input: every band plus terrain, or the read cache plus terrain when the cells
     *  stay in OPFS. */
    readonly inputBytes: number;
    /** The resident output: the whole map, or the sink's caches when it goes to disk instead. */
    readonly outputBytes: number;
    /** The sum of the three. An estimate, not a measurement — see the interface docs. */
    readonly peakBytes: number;
    /** The budget `fits` was measured against: 3 GiB unless `estimateMemory` was given an override.
     *  A desktop-shaped default; a phone's per-tab allowance is far lower. */
    readonly budgetBytes: number;
    /** wasm32's hard address space (4 GiB). Not the caller's to move. */
    readonly ceilingBytes: number;
    /** Negative when it does not fit — which is the number to show. */
    readonly headroomBytes: number;
    /** `peakBytes <= budgetBytes`. A verdict, with the confidence the interface docs describe. */
    readonly fits: boolean;
}

type Bridge = typeof import("./pkg/obc_web_assemble.js");

/** Memoized so concurrent callers share one fetch; cleared on failure so a transient network error
 *  can be retried rather than cached forever. */
let loading: Promise<Bridge> | null = null;

/**
 * Load and instantiate the wasm module, if it is not already up.
 *
 * `source` overrides where the `.wasm` comes from. Leave it out in the browser: the generated glue
 * resolves the module next to itself, which is the form the bundler rewrites to a hashed asset URL.
 * Node has no `fetch` for `file:` URLs, so tests pass the bytes directly.
 */
export function initAssemble(source?: InitInput): Promise<void> {
    if (!loading) {
        const pending = load(source);
        loading = pending;
        // Drop the memo if it settles as a failure, so the next call retries. Attached here so a
        // caller that ignores the returned promise cannot wedge the module into a failed state.
        pending.catch(() => {
            if (loading === pending) loading = null;
        });
    }
    return loading.then(() => undefined);
}

async function load(source?: InitInput): Promise<Bridge> {
    let mod: Bridge;
    try {
        mod = await import("./pkg/obc_web_assemble.js");
        await mod.default(source === undefined ? undefined : { module_or_path: source });
    } catch (cause) {
        throw new AssembleError(
            "internal",
            `The assembly module could not be loaded (${describe(cause)}). Check your connection and reload the page.`,
        );
    }
    return mod;
}

/**
 * Whether an assembly is in flight. One at a time: each run holds its inputs and its outputs in the
 * same 4 GiB linear memory, and two would abort the whole worker rather than throw. Set
 * synchronously, before the first `await`, so a caller that fires two off gets a diagnosable error.
 */
let assembling = false;

/**
 * The net under a forgotten {@link AssembleResult.release}. It holds the wasm handle, never the
 * result object, which would keep it alive forever and defeat the point. Collection happens
 * whenever the engine feels like it, so `release()` is still the contract.
 */
const abandoned =
    typeof FinalizationRegistry === "undefined"
        ? null
        : new FinalizationRegistry<{ free: () => void; name: string }>((held) => {
              console.warn(
                  `obc-web-assemble: an AssembleResult (${held.name}) was dropped without release(). Freeing it now — ` +
                      "but call release() when you are done with a map, or its bytes stay in wasm memory until a GC " +
                      "that may never come.",
              );
              try {
                  held.free();
              } catch {
                  // Already freed, or the module is gone. Either way there is nothing left to leak.
              }
          });

/**
 * Assemble `cells` into one `.obcm`.
 *
 * Run this in a Web Worker: the call blocks for the whole assembly, and the UI's cancel button is
 * `worker.terminate()`.
 *
 * Cells cross into wasm memory once as they are added, so the caller may drop its own references.
 * Or they do not cross at all: cells named in `sources` are read a block at a time for the length
 * of the run, and with `sink` the finished map is never in wasm memory either. The verify pass
 * always runs before this resolves. Only one assembly may be in flight; a second overlapping call
 * throws `internal`.
 *
 * @throws {AssembleError} carrying the engine's own message; see {@link AssembleErrorCode}.
 */
export async function assembleCells(
    cells: readonly AssembleCell[],
    schemaJson: string,
    skinJson: string,
    options: AssembleOptions = {},
    onProgress?: AssembleProgress,
    knownEmpty: readonly AssembleKnownEmpty[] = [],
    terrain?: { readonly lattice: AssembleTerrain; readonly cells: readonly AssembleTerrainCell[] },
    sources?: AssembleSources,
    sink?: AssembleMapSink,
    scratch?: AssembleScratchStore,
): Promise<AssembleResult> {
    if (assembling) {
        throw new AssembleError(
            "internal",
            "An assembly is already running. Two at once do not fit in one wasm heap — wait for the first to " +
                "resolve, or run the second in its own worker.",
        );
    }
    assembling = true;
    let assembler: InstanceType<Bridge["Assembler"]> | null = null;
    try {
        const mod = await ensure();
        assembler = new mod.Assembler(schemaJson, skinJson, JSON.stringify(options));
        for (const c of cells) {
            assembler.addCell(c.id, c.band, c.partial ?? false, c.bytes);
        }
        // Slots are the order these are added in. Asserted rather than assumed: a resolver keyed on
        // the wrong slot would read a valid cell and assemble a plausible wrong map.
        for (const [slot, c] of (sources?.cells ?? []).entries()) {
            const got = assembler.addCellByKey(c.id, c.band, c.partial ?? false, c.byteLength, c.key);
            if (got !== slot) {
                throw new AssembleError(
                    "internal",
                    `the assembler numbered cell ${c.id} slot ${got}, not ${slot} — the read callback would fetch ` +
                        "the wrong cell's bytes.",
                );
            }
        }
        for (const cell of knownEmpty) assembler.addKnownEmpty(cell.id, cell.band);
        if (terrain) {
            assembler.setTerrain(terrain.lattice.postingLog2, terrain.lattice.cellLog2);
            for (const cell of terrain.cells) assembler.addTerrainCell(cell.id, cell.sha256, cell.bytes);
        }
        if (sink) {
            // Checked before a byte is written: a half-wired sink is a defect in the caller
            // (`internal`), not a storage failure (`io`).
            for (const name of ["create", "write", "readAt", "seal", "sealed"] as const) {
                if (typeof sink[name] !== "function") {
                    throw new AssembleError(
                        "internal",
                        `the map sink has no ${name}() — a sink must provide create, write, readAt, seal and sealed.`,
                    );
                }
            }
        }
        const writes = sink
            ? {
                  create: () => sink.create(),
                  write: (bytes: Uint8Array) => sink.write(bytes),
                  readAt: (offset: number, into: Uint8Array) => sink.readAt(offset, into),
                  seal: () => sink.seal(),
                  sealed: (sha256: string, byteLength: number) => sink.sealed({ sha256, byteLength }),
              }
            : undefined;
        // Same as the sink: the methods are read off once, and a half-wired store is the caller's
        // defect rather than a storage failure.
        if (scratch) {
            for (const name of ["create", "append", "readAt", "len", "remove"] as const) {
                if (typeof scratch[name] !== "function") {
                    throw new AssembleError(
                        "internal",
                        `the scratch store has no ${name}() — a scratch store must provide create, append, readAt, ` +
                            `len and remove.`,
                    );
                }
            }
        }
        const spill = scratch
            ? {
                  create: () => scratch.create(),
                  append: (id: number, bytes: Uint8Array) => scratch.append(id, bytes),
                  readAt: (id: number, offset: number, into: Uint8Array) => scratch.readAt(id, offset, into),
                  len: (id: number) => scratch.len(id),
                  remove: (id: number) => scratch.remove(id),
              }
            : undefined;
        const summary = JSON.parse(assembler.run(onProgress, sources?.read, writes, spill)) as AssembleSummary;
        const warnings = assembler.warnings().map((w) => String(w));
        // Bound to the live assembler on purpose: `take()` is what frees the wasm-side copy, so the
        // caller decides when the bytes stop being wasm's problem. Nothing is copied until then.
        const owner = assembler;
        // Snapshotted so they keep answering after `take()` has moved the bytes out, and because a
        // taken map and a sunk one both report `false` from wasm.
        const sha256 = owner.fileSha256;
        const byteLength = owner.fileByteLength;
        const resident = owner.hasFile;
        let freed = false;
        const result: AssembleResult = {
            sha256,
            byteLength,
            resident,
            take: () => {
                try {
                    return owner.takeFile();
                } catch (cause) {
                    throw asAssembleError(cause);
                }
            },
            warnings,
            summary,
            release: () => {
                if (freed) return;
                freed = true;
                abandoned?.unregister(result);
                owner.free();
            },
        };
        abandoned?.register(result, { free: () => owner.free(), name: sha256.slice(0, 12) }, result);
        return result;
    } catch (cause) {
        // Only on the failure path: a successful assembly's `Assembler` stays alive because the
        // returned `take()` closure reads from it.
        assembler?.free();
        throw asAssembleError(cause);
    } finally {
        assembling = false;
    }
}

/**
 * Which escapes from linear memory the run being priced will have. Mirrors `Residency` in
 * `apps/obc-web-assemble/src/estimate.rs`.
 */
export interface Residency {
    /** The cells will stream from OPFS: a writable store with room and a passing sync-read probe.
     *  Only the worker can assert the probe half — see `assemble.worker.ts`. */
    readonly inputOnDisk: boolean;
    /** An {@link AssembleMapSink} will be wired in, so the finished file is never in wasm memory.
     *  Same caveat: it needs the worker's sync-handle probe, not just a browser that has OPFS. */
    readonly outputSunk: boolean;
}

/**
 * Project the peak memory of assembling a selection from the catalog's own byte totals, before the
 * download.
 *
 * `budgetBytes` overrides what the {@link MemoryEstimate.fits} verdict is measured against. The
 * default of 3 GiB is a desktop judgement; a caller that knows it is on a phone should pass what
 * that device will plausibly grant, because there the browser evicts the whole page under memory
 * pressure and the rider loses the download too. Anything non-finite or non-positive falls back.
 *
 * This loads the wasm module for what is three multiplications, so that the constants stay in one
 * place and cannot drift silently. The call site is the selection screen, which is about to need
 * the module anyway.
 */
export async function estimateMemory(
    networkBandBytes: number,
    totalCellBytes: number,
    terrainBytes: number,
    mergeBudgetBytes: number,
    residency: Residency,
    budgetBytes?: number,
): Promise<MemoryEstimate> {
    const mod = await ensure();
    return mod.obc_assemble_estimate(
        networkBandBytes,
        totalCellBytes,
        terrainBytes,
        mergeBudgetBytes,
        residency.inputOnDisk,
        residency.outputSunk,
        budgetBytes,
    ) as unknown as MemoryEstimate;
}

function ensure(): Promise<Bridge> {
    initAssemble();
    // `initAssemble` always assigns before returning; the assertion just tells TypeScript so.
    return loading as Promise<Bridge>;
}

const CODES: ReadonlySet<AssembleErrorCode> = new Set(ASSEMBLE_ERROR_CODES);

/** Whether a string the wasm side sent is one of the codes this module promises to throw. */
function isAssembleErrorCode(value: string): value is AssembleErrorCode {
    return (CODES as ReadonlySet<string>).has(value);
}

/**
 * Normalize whatever crossed the wasm boundary into an {@link AssembleError}. A value without a
 * known code is a wasm trap, an out-of-memory or a bug, reported as `internal` so that callers only
 * ever handle one error type.
 */
function asAssembleError(cause: unknown): AssembleError {
    if (cause instanceof AssembleError) return cause;
    if (typeof cause === "object" && cause !== null) {
        const { code, message } = cause as { code?: unknown; message?: unknown };
        if (typeof code === "string" && isAssembleErrorCode(code) && typeof message === "string") {
            return new AssembleError(code, message);
        }
    }
    return new AssembleError(
        "internal",
        `The assembly failed unexpectedly (${describe(cause)}). This is a bug — please report it with the selection.`,
    );
}

function describe(cause: unknown): string {
    if (cause instanceof Error) return cause.message;
    return String(cause);
}
