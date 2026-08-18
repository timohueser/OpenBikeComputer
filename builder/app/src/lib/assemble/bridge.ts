/**
 * Lazy wasm bridge to the shared Rust assembler. Assembly is one synchronous wasm
 * call and must run in a Worker; UI cancellation terminates that worker. Progress
 * posts between engine phases. All failures use {@link AssembleError}.
 *
 * **A map is one file.** The engine takes cells in and writes one `.obcm` — terrain
 * spliced into its tail — and that is the whole output: no manifest, no shards, no
 * roles. It also names nothing. What crosses this seam is a digest, a length and
 * (when the run buffered it) the bytes; whether that becomes `MAP.OBCM` on a card or
 * a save dialog's suggestion is the caller's decision, because the caller is the only
 * party that knows.
 */

import type { InitInput } from "./pkg/obc_web_assemble.js";

/**
 * Every code {@link AssembleErrorCode} can be, as a value — the runtime half of the same contract,
 * so the type and the set that guards the boundary cannot drift apart.
 *
 * `bridge.test.ts` pins this list against `ErrorCode::as_str` in
 * `apps/obc-web-assemble/src/driver.rs`, the way the phase list is pinned by running an assembly.
 */
export const ASSEMBLE_ERROR_CODES = ["input", "format", "capacity", "verify", "aborted", "io", "internal"] as const;

/**
 * Why an assembly failed. Mirrors `ErrorCode::as_str` in `apps/obc-web-assemble/src/driver.rs` —
 * the two are one contract, so add or rename in both.
 *
 * The first four are deliberately distinct because a caller's response to each differs:
 *
 * - `input` — OBCA §4.1: mixed schemas, an unaccepted hole, an unaccepted `partial` cell, a skin
 *   from another schema revision. **The selection is wrong**; the cells are fine.
 * - `format` — a cell does not honour the format. The download is corrupt, or the catalog served
 *   something that is not a cell.
 * - `capacity` — OBCA §5.7: a per-file ceiling. **Coverage must be reduced**, and the engine will
 *   never "solve" it by dropping any.
 * - `verify` — the §4.8 read-back rejected the output: the assembler wrote a map the real reader
 *   cannot read. A **defect**, never a retry: nothing was handed on, and nothing should be.
 * - `aborted` — the caller's own progress callback asked to stop.
 * - `io` — the byte source or sink failed.
 * - `internal` — a defect in the bridge, or the module failed to load. The message says so.
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
 * `apps/obc-web-assemble/src/driver.rs`.
 *
 * `nav` (the graph rewrite), `write` (the geometry graft) and `verify` (the §4.8 read-back) are the
 * long ones — measured at 20 %, 36 % and 43 % of a region-scale run (#1116's phase-D harness,
 * where the split went scale-free: both measured regions agree to half a point) — so a bar
 * that names the phase is worth having.
 */
export type AssemblePhase = "open" | "poi" | "nav" | "plan" | "write" | "verify" | "done";

/**
 * Progress sink. `fraction` is **overall** completion (0..1), weighted by the measured per-phase
 * split, not progress within `phase`.
 *
 * Return `true` to ask for an abort. It is honoured at the next write, so a request made during
 * `nav` — the engine's one long uninterruptible stretch — takes effect when that phase ends.
 */
export type AssembleProgress = (phase: AssemblePhase, fraction: number) => boolean | void;

/** One cell, as the catalog names it and the download delivered it. */
export interface AssembleCell {
    /** The canonical cell id, `<log2>/<i>/<j>`. */
    readonly id: string;
    /** The catalog's band id — not inferable from the bytes (OBCA §3.1). */
    readonly band: string;
    /** The catalog's `partial` flag (OBCA §3.7). */
    readonly partial?: boolean;
    readonly bytes: Uint8Array;
}

/**
 * One cell the caller keeps **outside** wasm memory and serves on demand (#1116 B2) — in the
 * browser, a file in OPFS read through a `FileSystemSyncAccessHandle`.
 *
 * Same identity as {@link AssembleCell}; instead of the bytes, the length the catalog published and
 * an opaque key. The key is the caller's own name for the bytes and is never interpreted here — it
 * appears in the message when a read fails. Reads themselves name a cell by its **slot**: its index
 * in the `cells` array passed to {@link AssembleSources}, because a number is what crosses the wasm
 * boundary cheaply.
 */
export interface AssembleSourceCell {
    /** The canonical cell id, `<log2>/<i>/<j>`. */
    readonly id: string;
    /** The catalog's band id — not inferable from the bytes (OBCA §3.1). */
    readonly band: string;
    /** The catalog's `partial` flag (OBCA §3.7). */
    readonly partial?: boolean;
    /** The catalog's byte count, which becomes the cell's length as the engine sees it. */
    readonly byteLength: number;
    /** Whatever the caller resolves reads against — the digest, for the OPFS store. */
    readonly key: string;
}

/**
 * Fill `into` with `into.byteLength` bytes at `offset` of the cell in `slot`. Return `true` on
 * success; **anything else fails the assembly** as `io`, naming the cell. A short read is a failure.
 *
 * Called from **inside** the synchronous assembly, so it must be synchronous itself — which is the
 * whole design: `FileSystemSyncAccessHandle.read()` is the one file read a browser has that can be
 * made from in there, and it only exists in a dedicated worker.
 *
 * `into` is a view onto wasm's linear memory, valid only for the duration of the call. Fill it and
 * return: do not keep it, do not pass it to anything asynchronous, and do not call back into the
 * assembler.
 *
 * It is called far less often than the engine reads — the wasm side serves the record-at-a-time
 * walks out of a 1 MiB block cache, so this is roughly one call per 64 KiB of a cell rather than one
 * per read. That ratio is what makes a per-call boundary crossing (~0.4 µs, measured in Node) and a
 * per-call file read affordable at country scale at all.
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
 * The terrain store's lattice, verbatim from the catalog's `terrain` block
 * (`OBCC_Spec.md` §13.1). Passing it is what gives the map a §1.3 terrain region at all.
 *
 * A catalog with no terrain block simply does not pass one, and the map assembles exactly as it did
 * before terrain existed — flat profiles, zero baked ascent, nothing missing (§13).
 */
export interface AssembleTerrain {
    readonly postingLog2: number;
    readonly cellLog2: number;
}

/**
 * One downloaded terrain cell: its id on the terrain grid, the digest the pinned terrain index
 * published, and the whole `.obcd` object.
 *
 * A **canonically void** square (open ocean, outside the dataset's coverage) is simply not passed —
 * it has no object at all (§13.6), and an absent cell reads identically to an all-`NODATA` one
 * (`OBCT_Spec.md` §4.3). That is why terrain needs no equivalent of {@link AssembleKnownEmpty}.
 */
export interface AssembleTerrainCell {
    readonly id: string;
    /** Lowercase-hex SHA-256 from the terrain index. The engine re-checks it before the block is
     *  copied — §4.8's posture: nothing self-made reaches a device unverified. */
    readonly sha256: string;
    readonly bytes: Uint8Array;
}

/** What an assembly can be told to do differently. Every field optional. */
export interface AssembleOptions {
    /** Proceed although a selected cell is missing (OBCA §4.1). */
    readonly acceptHoles?: boolean;
    /** Proceed although a cell is `partial` (OBCA §3.7). */
    readonly acceptPartial?: boolean;
    /** How much of a source cell one {@link AssembleRead} brings back (default 64 KiB, clamped to
     *  4 MiB). The cache holds sixteen of these, so it is also the input's whole residency ÷ 16.
     *
     *  `1` turns the cache **off** — one call per engine read. Nothing but a measurement should ask
     *  for that; it is here because "with the cache" only means something against "without". */
    readonly readBlockBytes?: number;
    /** The most memory the §4.6 nav merge's sorted passes may hold (default 64 MiB, floor 64 KiB).
     *
     *  It bounds what the merge *holds*, never what it writes — the same selection assembles to the
     *  same bytes at any budget. With an {@link AssembleScratchStore} wired, what it bounds is the
     *  buffer over an OPFS-backed spill, so lowering it genuinely lowers residency; without one the
     *  spill is in wasm memory and lowering it only moves the same bytes around. */
    readonly mergeBudgetBytes?: number;
}

/**
 * The finished map, as the {@link AssembleMapSink} is told about it: an identity with no bytes,
 * because the sink already wrote them and this side never held them.
 */
export interface AssembleSealedMap {
    /** Lowercase-hex SHA-256 of the whole file, the spliced raster included. */
    readonly sha256: string;
    /** How many bytes the sink was handed — what its file must be long. */
    readonly byteLength: number;
}

/**
 * Where the map itself goes, when the caller would rather wasm memory did not hold it (#1116 D1).
 * The write-side twin of {@link AssembleSources}, and the thing that decides whether a
 * country-scale map can be assembled in a tab at all.
 *
 * That was already the argument when a map was a set of shards, because the core shard could not be
 * split — one nav graph, one file. One file outright makes it bind harder rather than softer: a
 * DACH map is a single ~9 GiB object, which is not merely awkward in a 4 GiB wasm32 address space
 * but larger than the whole of it. At that scale a sink is not an optimisation; it is the only shape
 * in which the selection exists.
 *
 * In the browser this is one OPFS `FileSystemSyncAccessHandle`, opened in the assembly worker before
 * the run (see `../cells/store.ts`'s `openMapSink`). Everything below is called from **inside** the
 * synchronous assembly, so it must be synchronous itself — the same reason {@link AssembleRead} is.
 *
 * Passing one changes what the result carries: the bytes are the host's, so
 * {@link AssembleResult.resident} is `false` and {@link AssembleResult.take} throws. The identity
 * still crosses — through {@link AssembleMapSink.sealed}, and on the result itself.
 *
 * The contract, in four lines:
 *
 * - **Return `true`.** Anything falsy, or a throw, fails the run as `io`. A short write or a short
 *   read is a failure, not a partial success.
 * - **`bytes` and `into` are views onto wasm's linear memory**, valid only for the duration of the
 *   call. Fill or drain them and return; do not keep them, do not hand them to anything
 *   asynchronous, and do not call back into the assembler.
 * - **`seal` must flush.** The very next thing the engine does is read the map back.
 * - **A run that fails or is cancelled may have written most of a file.** Cleaning it up is the
 *   caller's job, and it is harmless meanwhile: a partial `.obcm` fails its own header checks, and
 *   nothing hands it on — `assembleCells` throws rather than return.
 */
export interface AssembleMapSink {
    /** Begin the map. Anything already there is superseded — a sink reusing a file must truncate
     *  it here. */
    create(): boolean;
    /** Append `bytes` to the map. */
    write(bytes: Uint8Array): boolean;
    /** Fill `into` with `into.byteLength` bytes at `offset` of the **sealed** map, for the §4.8
     *  read-back. Served through the wasm side's block cache, so this is called on the order of
     *  once per 64 KiB rather than once per engine read. */
    readAt(offset: number, into: Uint8Array): boolean;
    /** No more bytes are coming. Flush. */
    seal(): boolean;
    /** The map has passed §4.8 — here is what you have. Throwing fails the run as `io`: the file
     *  exists, and a caller that cannot record which bytes are in it must not report success. */
    sealed(map: AssembleSealedMap): void;
}

/**
 * Where the engine's **spill** goes instead of into wasm memory (#1116 D2's third seam) — the
 * sorted passes' working files, not the map's input or output. In the browser this is
 * `openScratchStore()`'s pool of OPFS sync access handles; every method runs inside the blocking
 * assembly, so all of them are synchronous and none may throw across the boundary on purpose.
 *
 * Without one the engine spills into linear memory — correct, byte-identical, and at exactly the
 * residency the spill exists to remove: from D3 on the spill is the merge's *edge stream*, which at
 * country scale is larger than the arrays it replaced. A worker that can serve sync handles should
 * always wire this.
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
    /** Lowercase-hex SHA-256 of the whole file — the map's identity, whether the bytes are here or
     *  a sink wrote them. */
    readonly sha256: string;
    /** The file's length. Readable without moving a byte, so a transfer can be planned before it is
     *  paid for, and it stays true after {@link AssembleResult.take} has emptied the buffer. */
    readonly byteLength: number;
    /** Whether the bytes are here to {@link AssembleResult.take}. `false` after a run with an
     *  {@link AssembleMapSink}: the file exists, it is simply the host's and not this module's to
     *  hand over. */
    readonly resident: boolean;
    /**
     * Move the map's bytes to JS and free the wasm copy. **Once.** A second call throws `internal`
     * rather than hand back an empty array: the natural retry shape (take, save, catch, take again)
     * would otherwise write a 0-byte file to a card and call it a map. Keep what the first call
     * returned.
     *
     * Throws for a sunk run too ({@link AssembleResult.resident} is `false`), and after
     * {@link AssembleResult.release}.
     */
    take(): Uint8Array;
    /** Everything OBCA says a producer SHOULD report rather than refuse. Ignoring these ships the
     *  same bytes; showing them tells the rider what the spec wanted them told. */
    readonly warnings: readonly string[];
    /** The engine's summary, in the shape `obcm-assemble --json` prints. */
    readonly summary: AssembleSummary;
    /**
     * Free whatever is still held in wasm memory: the assembler, and the map's bytes if they were
     * never taken. **Call this when you are done** — a map can be gigabytes, and wasm-bindgen
     * objects are not collected with their JS handles. `take()` after it throws.
     *
     * Idempotent. A result that is dropped without it is eventually freed by a
     * `FinalizationRegistry` net (with a `console.warn`), but "eventually" is the garbage
     * collector's word, not a plan: until then the map is still resident, and the next assembly is
     * competing with it for the same 4 GiB.
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
 * all", available **before** the download.
 *
 * This complements the OBCA §5.7 file-size ledger rather than repeating it: §5.7 prices the output
 * against the format's 4 GiB per-file ceiling; this prices the *run* against wasm32's 4 GiB address
 * space. A selection can pass one and fail the other. The model and its measured constants are
 * documented in `apps/obc-web-assemble/src/estimate.rs`.
 *
 * **How much to trust these numbers.** The engine term is a **linear fit through two measured
 * runs** — epic #1116's phase-resolved allocation harness on freiburg-regbez (90 MB of nav) and
 * Baden-Württemberg (296 MB), on the streamed engine — carried up with a stated margin. Nothing
 * larger than BW has been measured, and {@link MemoryEstimate.budgetBytes} is a judgement rather
 * than a measurement: browsers do not publish what they will grant, and an allocation wasm cannot
 * serve aborts the module outright, with no error to render. So: a comfortable `fits: false` is
 * reliable and is the case this exists for (refuse the impossible before a rider spends ten minutes
 * downloading it); a `fits: true` with little headroom means "probably", not "yes". Present a
 * near-budget verdict as a warning with the number, not as a green light — and never as a guarantee
 * to the user.
 *
 * **Where the limit sits.** With both escapes on — cells streaming from OPFS, the map written
 * through a sink — the resident terms are constants and the projection is engine-bound: it refuses
 * at roughly twice BW. A browser with no usable OPFS runs neither escape, holds the selection *and*
 * the finished file in linear memory, and binds far earlier. The {@link Residency} passed in states
 * which of the two this run will be, and the verdicts genuinely differ.
 */
export interface MemoryEstimate {
    /** The engine's working set — dominated by the nav rewrite. Measured 4.7 bytes resident per
     *  byte of selected `network` band, across two runs with margin (#1116). */
    readonly engineBytes: number;
    /** The **resident** input: every band plus terrain, or the read cache plus terrain when the
     *  cells stay in OPFS (#1116 B2). */
    readonly inputBytes: number;
    /** The **resident** output: the whole map (§4.8 needs the written bytes addressable), or the
     *  sink's write and read-back caches when it goes to disk instead (#1116 D1). */
    readonly outputBytes: number;
    /** The sum of the three. An estimate, not a measurement — see the interface docs. */
    readonly peakBytes: number;
    /** The budget `fits` was measured against: 3 GiB unless `estimateMemory` was given an override.
     *  A **desktop-shaped** default; a phone's per-tab allowance is far lower. */
    readonly budgetBytes: number;
    /** wasm32's hard address space (4 GiB). Not the caller's to move. */
    readonly ceilingBytes: number;
    /** Negative when it does not fit — which is the number to show. */
    readonly headroomBytes: number;
    /** `peakBytes <= budgetBytes`. A verdict, with the confidence the interface docs describe. */
    readonly fits: boolean;
}

type Bridge = typeof import("./pkg/obc_web_assemble.js");

/**
 * The in-flight or settled module load. Memoized so concurrent callers share one fetch; cleared on
 * failure so a transient network error can be retried rather than cached forever.
 */
let loading: Promise<Bridge> | null = null;

/**
 * Load and instantiate the wasm module, if it is not already up.
 *
 * `source` overrides where the `.wasm` comes from. Leave it out in the browser: the generated glue
 * resolves the module next to itself, which is the form the bundler rewrites to a hashed asset URL.
 * Node has no `fetch` for `file:` URLs, so tests (and any other non-browser host) pass the bytes
 * directly.
 *
 * Calling this early — say, when the selection is confirmed but the download has not finished —
 * turns the assembly's first moment into a plain function call.
 */
export function initAssemble(source?: InitInput): Promise<void> {
    if (!loading) {
        const pending = load(source);
        loading = pending;
        // Drop the memo if it settles as a failure, so the next call retries. Attached here (not in
        // the caller) so a caller that ignores the returned promise still cannot wedge the module
        // into a permanently-failed state.
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
 * Whether an assembly is in flight. One at a time, and the reason is arithmetic rather than
 * correctness: each run holds its inputs *and* its outputs in the same 4 GiB linear memory, and the
 * projection says a single country-scale assembly already spends three quarters of it. Two would
 * abort the module — which is not an exception a caller can catch, but the whole worker dying.
 *
 * Set synchronously, before the first `await`, so a caller that fires two off without awaiting the
 * first gets a diagnosable error rather than an interleaving.
 */
let assembling = false;

/**
 * The safety net under a forgotten {@link AssembleResult.release}. It holds the wasm handle (never
 * the result object — that would keep it alive forever and defeat the point), so when a result is
 * collected with the map still in wasm memory, it is freed and the omission is reported once.
 *
 * A net, not a mechanism: collection happens whenever the engine feels like it, and until then a
 * multi-gigabyte map is still resident. `release()` is still the contract.
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
 * **Run this in a Web Worker** — the call blocks for the whole assembly. See the threading contract
 * in this module's header, including why the UI's cancel button is `worker.terminate()`.
 *
 * Cells cross into wasm memory once, as they are added — the caller may drop its own references
 * immediately. Or they do not cross at all: pass `sources` and the cells named there are read a
 * block at a time, from wherever the caller keeps them, for the length of the run (#1116 B2). The
 * two forms may be mixed, and a caller uses whichever it has. The §4.8 verify pass runs before this
 * resolves either way, so a result is a map the real reader has already read back; there is
 * deliberately no way to skip it.
 *
 * The result holds the map in wasm memory until {@link AssembleResult.take}; call
 * {@link AssembleResult.release} when done, or it stays resident. Pass `sink` and it is never in
 * wasm memory at all (#1116 D1): the engine's bytes go straight to the caller's storage and the
 * §4.8 read-back comes back out of it. At country scale that is not an optimisation — the file is
 * larger than this address space. See {@link AssembleMapSink}.
 *
 * Only one assembly may be in flight at a time; a second overlapping call throws `internal`.
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
        // Slots are the order these are added in — the wasm side returns each one, and this asserts
        // rather than assumes, because a resolver keyed on the wrong slot would read a *valid* cell
        // and assemble a plausible wrong map.
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
        // The map sink crosses as one plain object with five methods — the wasm side reads them off
        // once, before the run — and `sealed` is adapted from wasm-bindgen's positional shape to the
        // one object a caller sees.
        if (sink) {
            // Checked here, before a byte is written, because the alternative is discovering it at
            // the first §4.8 read — a half-wired sink is a defect in the caller (`internal`), not a
            // storage failure (`io`), and telling the two apart matters more than usual when the
            // difference is "your code is wrong" versus "your disk is full".
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
        // The scratch store crosses the same way the sink does: one plain object, methods read off
        // once before the run. Checked here for the same reason — a half-wired store is the caller's
        // defect (`internal`), not a storage failure (`io`).
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
        // caller decides when the map's bytes stop being wasm's problem and start being the JS
        // heap's. Nothing is copied until then.
        const owner = assembler;
        // Snapshotted here so they keep answering after `take()` has moved the bytes out, and
        // because `resident` is a fact about the run rather than a live reading: a taken map and a
        // sunk one both report `false` from wasm, and only one of them was ever this module's.
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
 * Which of #1116 phase B's two escapes from linear memory the run being priced will actually have.
 * Mirrors `Residency` in `apps/obc-web-assemble/src/estimate.rs` — the model's own docs live there.
 */
export interface Residency {
    /** The cells will stream from OPFS (#1116 B2): a writable store with room **and** a passing
     *  sync-read probe. Resident input becomes the read cache plus terrain instead of the
     *  selection. Only the worker can assert the probe half — see `assemble.worker.ts`. */
    readonly inputOnDisk: boolean;
    /** An {@link AssembleMapSink} will be wired into the run (#1116 D1), so the finished file is
     *  never in wasm memory. Same caveat as `inputOnDisk`: it needs the worker's sync-handle probe,
     *  not just a browser that has OPFS. */
    readonly outputSunk: boolean;
}

/**
 * Project the peak memory of assembling a selection from the catalog's own byte totals, **before**
 * the download: the selected `network`-band cells (nav + POIs, no geometry), every selected cell
 * including the terrain squares, the terrain squares' own share, and the run's {@link Residency} —
 * because one selection can honestly fit the download path and not a direct device send.
 *
 * `budgetBytes` overrides the number the {@link MemoryEstimate.fits} verdict is measured against.
 * The default is 3 GiB, a **desktop** judgement; a caller that knows it is on a phone should pass
 * what that device will plausibly grant, because the mobile failure mode is worse than a slow tab —
 * the browser evicts the whole page under memory pressure and the rider loses the download too, with
 * no error to show for it. Anything non-finite or non-positive falls back to the default.
 *
 * **This loads the wasm module** — ~180 KB gzipped, for what is three multiplications. That is
 * deliberate: the constants live in exactly one place (`estimate.rs`, next to the benchmark they
 * were measured on) and a copy here is a copy that drifts silently, which for a *refusal threshold*
 * is the expensive kind of wrong. The cost is also mostly not wasted — the call site is the
 * selection screen, and a selection that fits is about to need this module anyway, so the fetch
 * doubles as the prefetch {@link initAssemble} exists to encourage. A caller that wants the estimate
 * without paying for it should gate on the catalog's byte totals first and only ask here once a
 * selection is plausible.
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
 * Normalize whatever crossed the wasm boundary into an {@link AssembleError}.
 *
 * The Rust side throws a real `Error` carrying `code`, so the happy path is a straight read. A value
 * without a known code is a wasm trap, an out-of-memory, or a bug — reported as `internal` rather
 * than passed through, so callers only ever handle one error type.
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
