<script lang="ts">
    // Step 3 on the cell catalog (#1038; mock R2·3): the coverage proof, the
    // final numbers, and the download-and-assemble run itself.
    //
    // Order of honesty, before a byte moves: the thumbnail shows the outline
    // the rider will live with — holes included — the figures show what it
    // costs, and two refusals can lock the button: the ledger's core-file
    // ceiling (§5.7, the sentence naming the navigation graph with both
    // figures), and the wasm memory projection (`estimateMemory`, run through
    // the worker *before* the download so an impossible assembly is refused
    // before anyone spends ten minutes fetching it — with a phone-sized budget
    // on a phone, where the failure mode is the browser evicting the page).
    //
    // The run: verified per-cell download (abortable) → assembly **in a Web
    // Worker** (the bridge's threading contract: one synchronous wasm call, so
    // cancel is `worker.terminate()`, progress crosses by postMessage, files
    // come back one at a time as transfers and go straight to the browser's
    // downloader).

    import { onDestroy } from "svelte";
    import type { AssemblePhase, MemoryEstimate } from "../../lib/assemble/bridge";
    import {
        isWorkerResponse,
        requestTransferList,
        type AssembleWorkerRequest,
        type WorkerCell,
    } from "../../lib/assemble/workerProtocol";
    import { saveBytes } from "../../lib/catalog/download";
    import {
        downloadCells,
        planCells,
        type CellDownloadProgress,
    } from "../../lib/catalog/v2/download";
    import { coverageRings, type RingPoint } from "../../lib/catalog/v2/outline";
    import type { UBox } from "../../lib/catalog/v2/grid";
    import { detailBandId, mergeMixedCellRects, parseCells, patchCount } from "../../lib/coverage/shape";
    import type { CoverageStore } from "../../lib/coverage/store.svelte";
    import { formatBytes } from "../../lib/format";

    let { store }: { store: CoverageStore } = $props();

    const ledger = $derived(store.ledger);
    const detailBand = $derived(detailBandId(store.catalog));

    // --- worker lifetime --------------------------------------------------

    let worker: Worker | null = null;

    function ensureWorker(): Worker {
        if (!worker) {
            worker = new Worker(new URL("../../lib/assemble/assemble.worker.ts", import.meta.url), {
                type: "module",
            });
            worker.onmessage = onWorkerMessage;
            // The worker's own code answers every failure with an `error`
            // message — these two fire for what that code never sees: the
            // script failing to boot at all (a lost chunk after a deploy) and
            // a message that could not be deserialized. Both arrive as the
            // protocol's `{code: "internal"}` shape so one switch handles
            // every failure, and the worker is dropped — a worker that could
            // not boot stays broken, so the next request must spawn fresh.
            worker.onerror = (e) => {
                workerFailed(e.message || "the assembly worker failed to start");
            };
            worker.onmessageerror = () => {
                workerFailed("a message to the assembly worker could not be delivered");
            };
        }
        return worker;
    }

    /** A worker-level failure (#1041 A4), routed like a posted `error`. */
    function workerFailed(message: string) {
        worker?.terminate();
        worker = null;
        onWorkerMessage(
            new MessageEvent("message", { data: { type: "error", code: "internal", message } }),
        );
    }

    onDestroy(() => worker?.terminate());

    // --- the run ----------------------------------------------------------

    type Phase = "idle" | "downloading" | "assembling" | "done" | "cancelled" | "error";
    let phase = $state<Phase>("idle");
    let dlProgress = $state<CellDownloadProgress | null>(null);
    let asmPhase = $state<AssemblePhase>("open");
    let asmFraction = $state(0);
    let savedFiles = $state<{ name: string; role: string; byteLength: number }[]>([]);
    let runWarnings = $state<string[]>([]);
    let errorMessage = $state<string | null>(null);
    let abortCtl: AbortController | null = null;

    const ASM_PHASE_LABEL: Record<AssemblePhase, string> = {
        open: "reading cells",
        poi: "merging places",
        nav: "stitching the road network",
        plan: "planning files",
        write: "writing",
        verify: "checking the result",
        manifest: "sealing the set",
        done: "done",
    };

    const ROLE_LABEL: Record<string, string> = {
        core: "routing + places",
        coarse: "zoomed-out overview",
        geometry: "map detail",
        manifest: "set manifest",
    };

    function onWorkerMessage(e: MessageEvent) {
        const msg = e.data as unknown;
        if (!isWorkerResponse(msg)) return;
        switch (msg.type) {
            case "estimate-result":
                estimate = msg.estimate;
                estimatePending = false;
                estimateError = null;
                break;
            case "progress":
                asmPhase = msg.phase;
                asmFraction = msg.fraction;
                break;
            case "file":
                // Straight to the browser's downloader, one file at a time —
                // nothing accumulates on this side of the boundary.
                saveBytes(msg.bytes, msg.name);
                savedFiles.push({ name: msg.name, role: msg.role, byteLength: msg.byteLength });
                break;
            case "done":
                runWarnings = msg.warnings;
                phase = "done";
                break;
            case "error":
                // Two conversations share this worker, and their failures are
                // different facts (#1041 A3): an error during a run belongs to
                // the run, but an error answering the background *estimate*
                // must not paint the screen with "Nothing was saved" about a
                // download that never started — it gets its own channel and
                // its own retry, and never wedges the button behind a pending
                // flag nothing will clear.
                if (phase === "assembling") {
                    errorMessage = msg.message;
                    phase = "error";
                } else {
                    estimateError = msg.message;
                }
                estimatePending = false;
                break;
        }
    }

    /** The set's 24-wire-byte display name, from the parts that made it. */
    function mapName(): string {
        const parts = store.selection.parts;
        const base = parts.length === 0 ? "OBC map" : parts[0].name;
        const name = parts.length > 1 ? `${base} +${parts.length - 1}` : base;
        let out = "";
        const encoder = new TextEncoder();
        for (const ch of name) {
            if (encoder.encode(out + ch).length > 24) break;
            out += ch;
        }
        return out;
    }

    async function run() {
        const resolution = store.resolution;
        const indices = store.indices;
        const l = ledger;
        if (!resolution || !indices || !l || phase === "downloading" || phase === "assembling") return;
        errorMessage = null;
        savedFiles = [];
        runWarnings = [];
        dlProgress = null;
        asmFraction = 0;
        phase = "downloading";

        const plan = planCells(resolution, store.catalog, indices);
        const cells: WorkerCell[] = [];
        abortCtl = new AbortController();
        try {
            await downloadCells(plan, {
                onCell: (item, bytes) => {
                    cells.push({ id: item.cell.id, band: item.band, partial: item.cell.partial, bytes });
                },
                onProgress: (p) => (dlProgress = p),
                signal: abortCtl.signal,
            });
        } catch (e) {
            if (abortCtl.signal.aborted) {
                phase = "cancelled";
            } else {
                errorMessage = e instanceof Error ? e.message : String(e);
                phase = "error";
            }
            return;
        }

        phase = "assembling";
        asmPhase = "open";
        const req: AssembleWorkerRequest = {
            type: "assemble",
            cells,
            schemaJson: store.rootBody,
            skinJson: JSON.stringify(store.skin),
            options: {
                name: mapName(),
                // Both were shown before this button unlocked. `acceptHoles`
                // is derived from the *shown* set, not the ledger's raw count
                // (#1041 A5): `store.holeCells()` is every band's holes — the
                // squares hatched on the map, the proof and the summary line —
                // so the assembly is never told to accept a hole the UI kept
                // to itself. Partial cells exist in essentially every real map
                // (every coarse cell of a country is partial, #1025).
                acceptHoles: store.holeCells().length > 0,
                acceptPartial: true,
            },
        };
        ensureWorker().postMessage(req, { transfer: requestTransferList(req) });
    }

    function cancel() {
        if (phase === "downloading") {
            abortCtl?.abort();
        } else if (phase === "assembling") {
            // The worker is blocked inside one synchronous wasm call and cannot
            // read a message — terminate IS the cancel (bridge threading
            // contract). Nothing is half-written: the set manifest goes last.
            worker?.terminate();
            worker = null;
            phase = "cancelled";
        }
    }

    // --- the memory projection, before the download -----------------------

    let estimate = $state<MemoryEstimate | null>(null);
    let estimatePending = $state(false);
    /** The estimate's own failure channel (#1041 A3) — never mixed into the
     *  run's. Cleared by the next request; retried by bumping the nonce. */
    let estimateError = $state<string | null>(null);
    let estimateNonce = $state(0);

    /** A phone's tab gets nowhere near a desktop's 3 GiB before the browser
     *  evicts the page — and eviction loses the download with no error to
     *  show. 1 GiB is the bridge test's own phone-shaped judgement. */
    const isMobileUa = typeof navigator !== "undefined" && /Android|iPhone|iPad|Mobile/i.test(navigator.userAgent);
    const MOBILE_BUDGET = 1024 ** 3;

    $effect(() => {
        void estimateNonce; // the retry button's lever
        const l = ledger;
        const idle = phase === "idle" || phase === "done" || phase === "cancelled" || phase === "error";
        if (!l || !l.isFinal || l.cellCount === 0 || !idle) {
            // Every exit clears the pending flag (#1041 A3): a selection that
            // empties or a run that starts must not leave "waiting for an
            // estimate" latched with nothing left to answer it.
            estimate = null;
            estimatePending = false;
            return;
        }
        const networkBandBytes = l.core.bytes;
        const totalCellBytes = l.totalBytes;
        estimatePending = true;
        estimateError = null;
        // Debounced: a slider mid-drag changes the figures every frame, and the
        // projection only matters once the selection settles.
        const timer = setTimeout(() => {
            ensureWorker().postMessage({
                type: "estimate",
                networkBandBytes,
                totalCellBytes,
                budgetBytes: isMobileUa ? MOBILE_BUDGET : undefined,
            } satisfies AssembleWorkerRequest);
        }, 500);
        return () => clearTimeout(timer);
    });

    const memoryRefusal = $derived.by(() => {
        if (!estimate || estimate.fits) return null;
        return (
            `Assembling this selection needs about ${formatBytes(estimate.peakBytes)} of browser memory — more than ` +
            `${isMobileUa ? "a phone's tab" : "a browser tab"} can be trusted with ` +
            `(${formatBytes(estimate.budgetBytes)}). The desktop app assembles the same selection natively.`
        );
    });

    const memoryCaution = $derived.by(() => {
        if (!estimate || !estimate.fits) return null;
        if (estimate.headroomBytes >= estimate.budgetBytes * 0.15) return null;
        return (
            `Close to the browser's memory budget: about ${formatBytes(estimate.peakBytes)} projected of ` +
            `${formatBytes(estimate.budgetBytes)}. It will probably assemble — the desktop app is the sure path.`
        );
    });

    // --- the coverage proof -----------------------------------------------

    interface Proof {
        viewW: number;
        viewH: number;
        coverage: string;
        holes: { x: number; y: number; w: number; h: number }[];
        routes: string[];
        holeCount: number;
        gapCount: number;
    }

    const proof = $derived.by<Proof | null>(() => {
        const resolution = store.resolution;
        const l = ledger;
        if (!resolution || !l || l.cellCount === 0) return null;
        const cellIds = resolution.cellsByBand.get(detailBand) ?? [];
        if (cellIds.length === 0) return null;
        const cells = parseCells(cellIds);
        const rings = coverageRings(cells);
        // Every band's holes, the same squares the map hatches (#1041 A5 —
        // `store.holeCells` owns the dedup and the reasoning).
        const holeCells = store.holeCells();
        const holeRects: UBox[] = mergeMixedCellRects(holeCells);
        const routes = store.selection.parts.flatMap((p) => (p.kind === "corridor" ? [p.points] : []));

        // Frame everything in an equirectangular plane, east-west corrected at
        // the middle latitude so shapes keep their look.
        let minLat = Infinity;
        let maxLat = -Infinity;
        let minLon = Infinity;
        let maxLon = -Infinity;
        const stretch = (lat: number, lon: number) => {
            if (lat < minLat) minLat = lat;
            if (lat > maxLat) maxLat = lat;
            if (lon < minLon) minLon = lon;
            if (lon > maxLon) maxLon = lon;
        };
        for (const ring of rings) for (const [lat, lon] of ring) stretch(lat, lon);
        for (const r of holeRects) {
            stretch(r.minLat, r.minLon);
            stretch(r.maxLat, r.maxLon);
        }
        for (const route of routes) for (const p of route) stretch(p.lat, p.lon);
        if (!Number.isFinite(minLat)) return null;

        const viewW = 330;
        const viewH = 200;
        const margin = 10;
        const kx = Math.cos((((minLat + maxLat) / 2) * Math.PI) / 180e6);
        const spanX = Math.max(1, (maxLon - minLon) * kx);
        const spanY = Math.max(1, maxLat - minLat);
        const scale = Math.min((viewW - 2 * margin) / spanX, (viewH - 2 * margin) / spanY);
        const ox = (viewW - spanX * scale) / 2;
        const oy = (viewH - spanY * scale) / 2;
        const px = (lon: number) => ox + (lon - minLon) * kx * scale;
        const py = (lat: number) => oy + (maxLat - lat) * scale;

        const ringPath = (ring: RingPoint[]) =>
            ring.map(([lat, lon], k) => `${k === 0 ? "M" : "L"}${px(lon).toFixed(1)} ${py(lat).toFixed(1)}`).join("") +
            "Z";

        return {
            viewW,
            viewH,
            coverage: rings.map(ringPath).join(""),
            holes: holeRects.map((r) => ({
                x: px(r.minLon),
                y: py(r.maxLat),
                w: (r.maxLon - r.minLon) * kx * scale,
                h: (r.maxLat - r.minLat) * scale,
            })),
            routes: routes.map(
                (route) =>
                    route
                        .map((p, k) => `${k === 0 ? "M" : "L"}${px(p.lon).toFixed(1)} ${py(p.lat).toFixed(1)}`)
                        .join(""),
            ),
            holeCount: holeCells.length,
            gapCount: Math.max(0, patchCount(cells) - 1),
        };
    });

    function proofCaption(p: Proof): string {
        const bits: string[] = [];
        if (p.holeCount) bits.push(`${p.holeCount} ${p.holeCount === 1 ? "hole" : "holes"}`);
        // Disjoint parts are a choice, not a defect (#1041 low sweep / A21):
        // someone who added Freiburg and Bremen knows they are apart, and "1
        // gap" tallied next to the holes read as a warning about it. The
        // mock's own tone — "gaps are fine — holes stay visible" — is the
        // caption's tone: reassure about the gaps, stay honest about holes.
        if (p.gapCount) bits.push("separate parts — that's fine");
        return bits.length ? `your map's coverage — ${bits.join(" · ")}` : "your map's coverage";
    }

    // --- gating -----------------------------------------------------------

    const running = $derived(phase === "downloading" || phase === "assembling");
    const refusal = $derived.by(() => {
        const l = ledger;
        if (!l || l.cellCount === 0) return null;
        if (l.verdict.kind === "refuse") return l.verdict.message;
        return memoryRefusal;
    });
    const ready = $derived.by(() => {
        const l = ledger;
        return (
            l !== null &&
            l.isFinal &&
            l.cellCount > 0 &&
            refusal === null &&
            !running &&
            !estimatePending &&
            // An unanswered projection keeps the mandatory pre-download check
            // honest: the button waits for the retry, not forever (A3).
            estimateError === null
        );
    });

    const dlPct = $derived(
        dlProgress && dlProgress.totalBytes > 0
            ? Math.min(100, Math.round((dlProgress.receivedBytes / dlProgress.totalBytes) * 100))
            : 0,
    );
</script>

{#if !ledger || ledger.cellCount === 0}
    <p class="line muted small">Nothing to download yet — add coverage in step 1.</p>
{:else}
    <div class="split">
        {#if proof}
            <figure class="proof">
                <svg viewBox="0 0 {proof.viewW} {proof.viewH}" role="img" aria-label={proofCaption(proof)}>
                    <defs>
                        <pattern id="proof-hatch" width="8" height="8" patternUnits="userSpaceOnUse">
                            <path d="M0 8 L8 0" stroke="var(--coral)" stroke-width="1.4" />
                        </pattern>
                    </defs>
                    <path
                        d={proof.coverage}
                        fill="var(--amber)"
                        fill-opacity="0.14"
                        fill-rule="evenodd"
                        stroke="var(--amber)"
                        stroke-width="1.6"
                    />
                    {#each proof.holes as r, k (k)}
                        <rect x={r.x} y={r.y} width={r.w} height={r.h} fill="url(#proof-hatch)" opacity="0.55" />
                        <rect
                            x={r.x}
                            y={r.y}
                            width={r.w}
                            height={r.h}
                            fill="none"
                            stroke="var(--coral)"
                            stroke-width="1"
                        />
                    {/each}
                    {#each proof.routes as d, k (k)}
                        <path {d} fill="none" stroke="var(--coral)" stroke-width="1.6" />
                    {/each}
                </svg>
                <figcaption class="small faint">{proofCaption(proof)}</figcaption>
            </figure>
        {/if}

        <div class="facts">
            <p class="line">
                <span class="serif">{store.catalog.schema.name}</span> schema ·
                <span class="serif">{store.skin.name}</span> skin
            </p>
            <p class="line mono small">
                {#if ledger.isFinal}
                    {formatBytes(ledger.totalBytes)} · {ledger.cellCount}
                    {ledger.cellCount === 1 ? "cell" : "cells"} · assembled on this computer
                {:else}
                    pricing…
                {/if}
            </p>

            {#if refusal}
                <p class="line warn small">{refusal}</p>
            {:else if estimateError}
                <p class="line warn small">
                    Couldn't project the memory this assembly needs — the check runs before any
                    download: {estimateError}
                    <button type="button" class="retry" onclick={() => (estimateNonce += 1)}
                        >retry</button
                    >
                </p>
            {:else if memoryCaution}
                <p class="line caution small">{memoryCaution}</p>
            {/if}

            {#if phase === "idle" || phase === "cancelled" || phase === "error" || phase === "done"}
                <button type="button" class="btn primary" disabled={!ready} onclick={run}>
                    {ledger.isFinal ? `Download map (${formatBytes(ledger.totalBytes)})` : "Download map"}
                </button>
            {:else}
                <div class="runrow">
                    <button type="button" class="btn" onclick={cancel}>Cancel</button>
                    {#if phase === "downloading" && dlProgress}
                        <span class="small muted">
                            downloading cells — {dlProgress.completedCells}/{dlProgress.totalCells} ·
                            {formatBytes(dlProgress.receivedBytes)} of {formatBytes(dlProgress.totalBytes)}
                        </span>
                    {:else if phase === "assembling"}
                        <span class="small muted">
                            assembling — {ASM_PHASE_LABEL[asmPhase]} · {Math.round(asmFraction * 100)}%
                        </span>
                    {/if}
                </div>
                <div class="bar">
                    <span
                        style:width={`${phase === "downloading" ? dlPct : Math.round(asmFraction * 100)}%`}
                        class:assembling={phase === "assembling"}
                    ></span>
                </div>
            {/if}

            {#if phase === "done"}
                <div class="done">
                    <p class="line small">
                        Saved {savedFiles.length}
                        {savedFiles.length === 1 ? "file" : "files"} — copy
                        {savedFiles.length === 1 ? "it" : "all of them"} to the top level of the device's
                        card.
                    </p>
                    <ul class="files mono small">
                        {#each savedFiles as f (f.name)}
                            <li>
                                {f.name}
                                <span class="faint">
                                    · {ROLE_LABEL[f.role] ?? f.role} · {formatBytes(f.byteLength)}</span
                                >
                            </li>
                        {/each}
                    </ul>
                    {#if savedFiles.length > 1}
                        <p class="line faint small">
                            Your browser may ask to allow multiple downloads — the map is a set, and every
                            file of it matters.
                        </p>
                    {/if}
                    {#each runWarnings as w, k (k)}
                        <p class="line caution small">{w}</p>
                    {/each}
                </div>
            {:else if phase === "error"}
                <p class="line warn small">Nothing was saved: {errorMessage}</p>
            {:else if phase === "cancelled"}
                <p class="line faint small">Cancelled — nothing was saved.</p>
            {:else if phase === "idle"}
                <p class="line faint small">
                    Cells are verified against the catalog (SHA-256), then assembled and read back in full
                    before anything reaches the card.
                </p>
            {/if}
        </div>
    </div>
{/if}

<style>
    .split {
        display: flex;
        gap: 14px;
        align-items: flex-start;
        flex-wrap: wrap;
    }

    .proof {
        margin: 0;
        flex: 0 1 240px;
        min-width: 180px;
    }

    .proof svg {
        width: 100%;
        height: auto;
        background: var(--parchment-2);
        border: 1px solid var(--parchment-3);
        border-radius: 8px;
    }

    .proof figcaption {
        text-align: center;
        margin-top: 4px;
    }

    .facts {
        flex: 1 1 240px;
        min-width: 220px;
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .line {
        margin: 0;
        line-height: 1.45;
    }

    .serif {
        font-family: var(--serif);
        font-style: italic;
    }

    .line.warn {
        color: var(--coral);
    }

    .retry {
        background: none;
        border: none;
        color: var(--forest);
        text-decoration: underline;
        padding: 0;
        font-size: inherit;
    }

    .line.caution {
        background: rgba(227, 173, 51, 0.18);
        border: 1px solid var(--amber);
        border-radius: 8px;
        padding: 6px 9px;
    }

    .btn {
        align-self: flex-start;
    }

    .runrow {
        display: flex;
        align-items: center;
        gap: 10px;
    }

    .bar {
        height: 6px;
        border-radius: 999px;
        background: var(--parchment-2);
        overflow: hidden;
    }

    .bar span {
        display: block;
        height: 100%;
        background: var(--forest);
        transition: width 0.2s;
    }

    @media (prefers-reduced-motion: reduce) {
        .bar span {
            transition: none;
        }
    }

    .bar span.assembling {
        background: var(--wood);
    }

    .done {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .files {
        list-style: none;
        margin: 0;
        padding: 0;
    }
</style>
