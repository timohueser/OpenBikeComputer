/**
 * Lazy wasm bridge to the shared Rust assembler. Assembly is one synchronous
 * wasm call and must run in a Worker; UI cancellation terminates that worker.
 * Progress posts between engine phases, and completed files should cross back
 * as transferable buffers. All failures use {@link AssembleError}.
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
 * - `verify` — the §4.8 read-back rejected the output: the assembler wrote a set the real reader
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
 * long ones — measured at 16 %, 24 % and 60 % of a region-scale run (#1116's harness) — so a bar
 * that names the phase is worth having.
 */
export type AssemblePhase = "open" | "poi" | "nav" | "plan" | "write" | "verify" | "manifest" | "done";

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
 * (`OBCC_Spec.md` §13.1). Passing it is what makes the set carry a `terrain` role at all.
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
 * it has no object at all (§13.6), and in the shard an absent cell and an all-`NODATA` one answer
 * identically (`OBCT_Spec.md` §4.3). That is why terrain needs no equivalent of
 * {@link AssembleKnownEmpty}.
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
    /** The set's display name, 24 bytes on the wire (OBCA §5.2). */
    readonly name?: string;
    /** The id the derived filenames use, 0..=999 (default 1). */
    readonly cardId?: number;
    /** Split a geometry shard wherever it exceeds this (default 1 GiB). */
    readonly targetShardBytes?: number;
    /** Proceed although a selected cell is missing (OBCA §4.1). */
    readonly acceptHoles?: boolean;
    /** Proceed although a cell is `partial` (OBCA §3.7). */
    readonly acceptPartial?: boolean;
    /** Write a role-partitioned set even when the map would fit one file — smaller files are better
     *  resumable upload units. */
    readonly forceSplit?: boolean;
    /** How much of a source cell one {@link AssembleRead} brings back (default 64 KiB, clamped to
     *  4 MiB). The cache holds sixteen of these, so it is also the input's whole residency ÷ 16.
     *
     *  `1` turns the cache **off** — one call per engine read. Nothing but a measurement should ask
     *  for that; it is here because "with the cache" only means something against "without". */
    readonly readBlockBytes?: number;
}

/**
 * One shard, handed over the moment its §4.8 read-back passed — mid-assembly, from inside the
 * blocking call (#1116 B1).
 *
 * Its bytes are **already out of wasm memory** when this arrives: the wasm-side buffer is freed
 * before the callback runs, which is the entire point. What it costs is that the sink has to be
 * synchronous. See {@link AssembleFileSink}.
 */
export interface AssembleStreamedFile {
    /** The derived 8.3 filename (`MS<id>S<kk>.OBM`). */
    readonly name: string;
    /** Always a shard role — the terrain shard and the OBCS manifest are not streamed. */
    readonly role: "core" | "coarse" | "geometry";
    /** Lowercase-hex SHA-256, the same digest the manifest will record for it. */
    readonly sha256: string;
    readonly bytes: Uint8Array;
}

/**
 * Where streamed shards go. Passing one is what turns streaming on, and it changes what the result
 * contains: a shard the sink took is **not** in {@link AssembleResult.files} — only the terrain
 * shard and the manifest are left there.
 *
 * The contract is narrow, because this runs *inside* the synchronous assembly:
 *
 * - **Do not block.** The assembly is stopped behind it and nothing can be awaited. Post the buffer
 *   on (transfer it) or park it; do the slow part after the run.
 * - **Do not throw.** A throw fails the run as `io` — by then the bytes have already left wasm, and
 *   a set with a hole in it must never be reported as finished.
 * - **A run that fails or is cancelled may already have called it.** Cleaning up what was handed
 *   over is the caller's job. Nothing half-usable can reach a device either way: the OBCS manifest
 *   is written last (OBCA §5.4), so a set without one is not a map.
 */
export type AssembleFileSink = (file: AssembleStreamedFile) => void;

/**
 * One finished file. `take()` moves its bytes out of wasm memory and frees the wasm-side copy, so
 * call it once, and one file at a time: an assembled set can be gigabytes.
 */
export interface AssembledFile {
    /** The derived 8.3 filename (`MS<id>S<kk>.OBM`, `MS<id>.OBD`, `MS<id>.OBS`). */
    readonly name: string;
    readonly role: "core" | "coarse" | "geometry" | "terrain" | "manifest";
    /** Lowercase-hex SHA-256 as the manifest records it; empty for the manifest itself. */
    readonly sha256: string;
    /** The size at the moment the assembly finished — a transfer can be planned before it is paid
     *  for. It does not change when the file is taken; the bytes do. */
    readonly byteLength: number;
    /**
     * Move the bytes to JS and free the wasm copy. **Once.** A second call throws `internal` rather
     * than hand back an empty array: the natural retry shape (take, upload, catch, take again) would
     * otherwise write a 0-byte shard to a card and call it a map. Keep what the first call returned.
     *
     * Also throws after {@link AssembleResult.release}.
     */
    take(): Uint8Array;
}

/** What an assembly produced. Files come shards-first with the OBCS manifest **last** (OBCA §5.4). */
export interface AssembleResult {
    /** Everything an {@link AssembleFileSink} did not already take. With no sink, the whole set. */
    readonly files: readonly AssembledFile[];
    /** Everything OBCA says a producer SHOULD report rather than refuse. Ignoring these ships the
     *  same bytes; showing them tells the rider what the spec wanted them told. */
    readonly warnings: readonly string[];
    /** The engine's summary, in the shape `obcm-assemble --json` prints. */
    readonly summary: AssembleSummary;
    /**
     * Free everything still held in wasm memory: the assembler and any file whose bytes were not
     * taken. **Call this when you are done** — a set can be gigabytes, and wasm-bindgen objects are
     * not collected with their JS handles. `take()` on a released file throws.
     *
     * Idempotent. A result that is dropped without it is eventually freed by a
     * `FinalizationRegistry` net (with a `console.warn`), but "eventually" is the garbage
     * collector's word, not a plan: until then the set is still resident, and the next assembly is
     * competing with it for the same 4 GiB.
     */
    release(): void;
}

/** The summary document, as far as callers rely on it. Additional fields exist; see the CLI. */
export interface AssembleSummary {
    readonly cells: number;
    readonly bytes: number;
    readonly manifest: string;
    readonly shards: readonly {
        readonly index: number;
        readonly role: string;
        readonly file: string;
        readonly bytes: number;
        readonly sha256: string;
        readonly verified: { chunks: number; features: number; nav_nodes: number } | null;
    }[];
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
 * **Where the limit sits.** With #1116 phase B's two escapes on — cells streaming from OPFS,
 * shards handed out after their verify — the resident terms are constants and the projection is
 * engine-bound: it refuses at roughly twice BW on the download path. The device path keeps the
 * whole set until `planned` and binds earlier, at ~1.4× BW; a browser with no usable OPFS runs the
 * pre-B shape and binds earlier still. The {@link Residency} passed in states which of these this
 * run will be, and the verdicts genuinely differ — compute one per destination, never one for all.
 */
export interface MemoryEstimate {
    /** The engine's working set — dominated by the nav rewrite. Measured 4.7 bytes resident per
     *  byte of selected `network` band, across two runs with margin (#1116). */
    readonly engineBytes: number;
    /** The **resident** input: every band plus terrain, or the read cache plus terrain when the
     *  cells stay in OPFS (#1116 B2). */
    readonly inputBytes: number;
    /** The **resident** output: the whole set (§4.8 needs written bytes addressable, and the device
     *  path keeps them), or one shard plus the terrain sink when shards stream out (#1116 B1). */
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
 * collected with files still in wasm memory, they are freed and the omission is reported once.
 *
 * A net, not a mechanism: collection happens whenever the engine feels like it, and until then a
 * multi-gigabyte set is still resident. `release()` is still the contract.
 */
const abandoned =
    typeof FinalizationRegistry === "undefined"
        ? null
        : new FinalizationRegistry<{ free: () => void; name: string }>((held) => {
              console.warn(
                  `obc-web-assemble: an AssembleResult (${held.name}) was dropped without release(). Freeing it now — ` +
                      "but call release() when you are done with a set, or its bytes stay in wasm memory until a GC " +
                      "that may never come.",
              );
              try {
                  held.free();
              } catch {
                  // Already freed, or the module is gone. Either way there is nothing left to leak.
              }
          });

/**
 * Assemble `cells` into one `.obcm` or a volume set.
 *
 * **Run this in a Web Worker** — the call blocks for the whole assembly. See the threading contract
 * in this module's header, including why the UI's cancel button is `worker.terminate()`.
 *
 * Cells cross into wasm memory once, as they are added — the caller may drop its own references
 * immediately. Or they do not cross at all: pass `sources` and the cells named there are read a
 * block at a time, from wherever the caller keeps them, for the length of the run (#1116 B2). The
 * two forms may be mixed, and a caller uses whichever it has. The §4.8 verify pass runs before this
 * resolves either way, so a result is a set the real reader has already read back; there is
 * deliberately no way to skip it.
 *
 * The result keeps its files in wasm memory until each is `take()`n; call {@link
 * AssembleResult.release} when done, or the set stays resident. Pass `onFile` and that stops being
 * true of the shards: each one leaves wasm as soon as it is verified, so the output's contribution
 * to the peak is one shard rather than the whole set (#1116 B1). Read {@link AssembleFileSink}
 * before you do — the sink runs inside the blocking call and may neither block nor throw.
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
    onFile?: AssembleFileSink,
    sources?: AssembleSources,
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
        // The wasm side calls this once per shard, synchronously, with the bytes already out of its
        // linear memory. It is adapted rather than passed through so the sink sees one object and
        // the four-argument wasm-bindgen shape stays an implementation detail.
        const sink = onFile
            ? (name: string, role: string, sha256: string, bytes: Uint8Array) =>
                  onFile({ name, role: role as AssembleStreamedFile["role"], sha256, bytes })
            : undefined;
        const summary = JSON.parse(assembler.run(onProgress, sink, sources?.read)) as AssembleSummary;
        const warnings = assembler.warnings().map((w) => String(w));
        // Bound to the live assembler on purpose: `take()` is what frees the wasm-side copy, so the
        // caller decides when each file's bytes stop being wasm's problem and start being the JS
        // heap's. Nothing is copied until then.
        const owner = assembler;
        const files: AssembledFile[] = [];
        for (let i = 0; i < owner.fileCount; i++) {
            files.push({
                name: owner.fileName(i),
                role: owner.fileRole(i) as AssembledFile["role"],
                sha256: owner.fileSha256(i),
                // Snapshotted here, so it keeps reporting the file's real size after `take()` has
                // moved the bytes out and the wasm-side length has become 0.
                byteLength: owner.fileByteLength(i),
                take: () => {
                    try {
                        return owner.takeFile(i);
                    } catch (cause) {
                        throw asAssembleError(cause);
                    }
                },
            });
        }
        let freed = false;
        const result: AssembleResult = {
            files,
            warnings,
            summary,
            release: () => {
                if (freed) return;
                freed = true;
                abandoned?.unregister(result);
                owner.free();
            },
        };
        abandoned?.register(result, { free: () => owner.free(), name: summary.manifest }, result);
        return result;
    } catch (cause) {
        // Only on the failure path: a successful assembly's `Assembler` stays alive because the
        // returned `take()` closures read from it.
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
    /** `> 0`: shards are taken mid-run at this split size (#1116 B1) — the download path. `0`: the
     *  set is kept until the run ends — the device path, which needs `planned`'s counts first. */
    readonly streamedShardBytes: number;
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
    residency: Residency,
    budgetBytes?: number,
): Promise<MemoryEstimate> {
    const mod = await ensure();
    return mod.obc_assemble_estimate(
        networkBandBytes,
        totalCellBytes,
        terrainBytes,
        residency.inputOnDisk,
        residency.streamedShardBytes,
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
