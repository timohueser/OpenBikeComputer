/**
 * The browser side of the **assembly bridge** (epic #1016, P4b — issue #1034).
 *
 * Downloaded OBCA cells in, one `.obcm` — or a volume set's shards plus its OBCS manifest — out,
 * assembled client-side by `apps/obc-web-assemble` compiled to wasm. There is no TypeScript
 * re-implementation here on purpose: the bytes a visitor ends up putting on a card are produced by
 * the same `obcm-assemble` engine the CLI runs, and `bridge.test.ts` pins that equality against the
 * checked-in fixture the native CLI produced.
 *
 * The wasm module is fetched through a **dynamic import**, so its ~180 KB (gzipped) of glue +
 * module land in their own bundle chunk and cost nothing until someone actually assembles a map.
 *
 * Everything this module throws is an {@link AssembleError} — including a failed load or an
 * unexpected wasm trap, which arrive as `code: "internal"`. Callers never have to guess at a
 * message shape.
 *
 * **This file is plumbing, not UI.** It is the typed surface P4c builds the assembly screen on; it
 * holds no state beyond the memoized module load and a guard against two assemblies at once.
 *
 * ## Threading: this must run in a Web Worker
 *
 * {@link assembleCells} looks `async`, but the assembly itself is **one synchronous wasm call**.
 * Nothing yields inside it: a country-scale run is ~20 s of straight-line compute (PR #1027's
 * switzerland benchmark, 410 cells / 717 MB → 19.9 s), and the §4.6 nav rewrite alone is ~4 s. On
 * the main thread that is a frozen tab — no repaint, no click handling, no spinner animation, and
 * on some browsers a "page unresponsive" prompt. **Call this from a dedicated Worker.** (P4c owns
 * the worker itself; this paragraph is the contract it is written against.)
 *
 * What follows from that, and it is the part a cancel button depends on:
 *
 * - **The UI's cancel is `worker.terminate()`.** The worker is blocked inside wasm for the whole
 *   run, so it cannot receive a `postMessage` while the assembly is in flight — a cooperative flag
 *   set from the main thread would only be read after the thing it was meant to stop had finished.
 *   Terminating drops the worker's whole heap, which is exactly what an abandoned multi-gigabyte
 *   assembly wants. Nothing is left half-written: the OBCS manifest is written last (OBCA §5.4), so
 *   there is no set to clean up. (Reading a flag mid-run *is* possible with a `SharedArrayBuffer`,
 *   but that needs the page served cross-origin-isolated — COOP `same-origin` + COEP
 *   `require-corp` — which the hosted builder is not, and which would constrain every other asset
 *   the page loads. Not worth it for a button that `terminate()` already implements.)
 * - **The cooperative abort still has a job**, just not that one: it is for policies the worker runs
 *   on *itself*, from inside the callback — a memory watchdog, a deadline, a "the tab went hidden
 *   an hour ago" rule. That is why {@link AssembleProgress} returns `boolean | void`, and why an
 *   abort surfaces as a clean `aborted` error rather than a dead worker.
 * - **Progress crosses by `postMessage`.** The callback runs on the worker thread between wasm
 *   steps, so posting `{phase, fraction}` from it is enough; the main thread renders. Structured
 *   clone of a small object ~100 times a run costs nothing.
 * - **Hand the finished files over as transfers.** `take()` returns a `Uint8Array` the worker owns;
 *   post it with its buffer in the transfer list, one file at a time, or the set is copied.
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
 * `nav` (the graph rewrite) and `verify` (the §4.8 read-back) are the two long ones — measured at
 * 20 % and 74 % of a country-scale run — so a bar that names the phase is worth having.
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

/** A selected cell with canonical empty content and therefore no OBCM bytes. */
export interface AssembleKnownEmpty {
    readonly id: string;
    readonly band: string;
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
}

/**
 * One finished file. `take()` moves its bytes out of wasm memory and frees the wasm-side copy, so
 * call it once, and one file at a time: an assembled set can be gigabytes.
 */
export interface AssembledFile {
    /** The derived 8.3 filename (`MS<id>S<kk>.OBM`, `MS<id>.OBS`). */
    readonly name: string;
    readonly role: "core" | "coarse" | "geometry" | "manifest";
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
 * **How much to trust these numbers.** The model is a **linear extrapolation from a single measured
 * run** (PR #1027's switzerland: 410 cells / 717 MB, peak RSS 1.73 GB), and {@link
 * MemoryEstimate.budgetBytes} is a judgement rather than a measurement — browsers do not publish
 * what they will grant, and an allocation wasm cannot serve aborts the module outright, with no
 * error to render. So: a comfortable `fits: false` is reliable and is the case this exists for
 * (refuse the impossible before a rider spends ten minutes downloading it); a `fits: true` with
 * little headroom means "probably", not "yes". Present a near-budget verdict as a warning with the
 * number, not as a green light — and never as a guarantee to the user.
 */
export interface MemoryEstimate {
    /** The engine's working set — dominated by the nav rewrite. Measured 6.4 bytes resident per
     *  byte of rebuilt nav section, on one run. */
    readonly engineBytes: number;
    /** The downloaded cells, resident for the whole run. */
    readonly inputBytes: number;
    /** The assembled set, resident until each file is taken (§4.8 needs it addressable). */
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
 * immediately. The §4.8 verify pass runs before this resolves, so a result is a set the real reader
 * has already read back; there is deliberately no way to skip it.
 *
 * The result keeps its files in wasm memory until each is `take()`n; call {@link
 * AssembleResult.release} when done, or the set stays resident.
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
        for (const cell of knownEmpty) assembler.addKnownEmpty(cell.id, cell.band);
        const summary = JSON.parse(assembler.run(onProgress)) as AssembleSummary;
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
 * Project the peak memory of assembling a selection from the catalog's own byte totals, **before**
 * the download: the selected `network`-band cells (nav + POIs, no geometry) and every selected cell.
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
    budgetBytes?: number,
): Promise<MemoryEstimate> {
    const mod = await ensure();
    return mod.obc_assemble_estimate(networkBandBytes, totalCellBytes, budgetBytes) as unknown as MemoryEstimate;
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
