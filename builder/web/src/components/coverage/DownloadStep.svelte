<script lang="ts">
    // Final coverage proof, then the controls of one map run (`MapRun`): download the map, or
    // send it to a connected device.

    import { onDestroy, onMount } from "svelte";
    import type { AssemblePhase } from "../../lib/assemble/bridge";
    import { coverageRings, type RingPoint } from "../../lib/catalog/outline";
    import type { UBox } from "../../lib/core/grid";
    import { MapRun } from "../../lib/coverage/run.svelte";
    import { detailBandId, mergeMixedCellRects, parseCells, patchCount } from "../../lib/coverage/shape";
    import type { CoverageStore } from "../../lib/coverage/store.svelte";
    import type { SendAssembledMap } from "../../lib/device/write";
    import { formatBytes } from "../../lib/format";

    let {
        store,
        onSendReadyChange,
        onFailureChange,
        onRefreshBlockedChange,
    }: { store: CoverageStore; onSendReadyChange?: (ready: boolean) => void; onFailureChange?: (failed: boolean) => void; onRefreshBlockedChange?: (blocked: boolean) => void } = $props();

    // `CoverageHome` mounts a new step for each new store.
    // svelte-ignore state_referenced_locally
    const run = new MapRun(store);

    const ledger = $derived(store.ledger);
    const detailBand = $derived(detailBandId(store.catalog));

    onDestroy(() => {
        run.destroy();
        onSendReadyChange?.(false);
    });

    onMount(() => run.start());

    const ASM_PHASE_LABEL: Record<AssemblePhase, string> = {
        open: "reading cells",
        poi: "merging places",
        nav: "stitching the road network",
        plan: "planning the file",
        write: "writing",
        verify: "checking the result",
        done: "done",
    };

    export const sendToDevice: SendAssembledMap = run.sendToDevice;

    export function pauseForRefresh(): Promise<() => void> {
        return run.pauseForRefresh();
    }

    $effect(() => onFailureChange?.(run.phase === "error"));
    $effect(() => onRefreshBlockedChange?.(run.refreshBlocked));
    $effect(() => onSendReadyChange?.(run.ready));

    interface Proof {
        viewW: number;
        viewH: number;
        coverage: string;
        holes: { x: number; y: number; w: number; h: number }[];
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
        // Every band's holes, the same squares the map hatches; `store.holeCells` owns
        // the dedup and the reasoning.
        const holeCells = store.holeCells();
        const holeRects: UBox[] = mergeMixedCellRects(holeCells);

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
            holeCount: holeCells.length,
            gapCount: Math.max(0, patchCount(cells) - 1),
        };
    });

    function proofCaption(p: Proof): string {
        const bits: string[] = [];
        if (p.holeCount) bits.push(`${p.holeCount} ${p.holeCount === 1 ? "hole" : "holes"}`);
            // Disjoint parts are a choice, not a defect: someone who added Freiburg and
            // Bremen knows they are apart, and "1 gap" tallied next to the holes read as
            // a warning about it. Reassure about the gaps, stay honest about holes.
        if (p.gapCount) bits.push("separate parts — that's fine");
        return bits.length ? `your map's coverage — ${bits.join(" · ")}` : "your map's coverage";
    }

    const dlPct = $derived(
        run.dlProgress && run.dlProgress.totalBytes > 0
            ? Math.min(100, Math.round((run.dlProgress.receivedBytes / run.dlProgress.totalBytes) * 100))
            : 0,
    );
</script>

{#if !ledger || ledger.cellCount === 0}
    <p class="line muted small">Choose a region or draw an area on the map.</p>
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
                </svg>
                <figcaption class="small faint">{proofCaption(proof)}</figcaption>
            </figure>
        {/if}

        <div class="facts">
            {#if run.refusal}
                <p class="line warn small">{run.refusal}</p>
            {:else if run.estimateError}
                <p class="line warn small">
                    Could not check memory requirements: {run.estimateError}
                    <button type="button" class="retry" onclick={() => run.retryEstimate()}
                        >retry</button
                    >
                </p>
            {:else if run.memoryCaution}
                <p class="line caution small">{run.memoryCaution}</p>
            {/if}

            <div class="cell-storage small">
                <label>
                    <input
                        type="checkbox"
                        checked={run.keepCells}
                        disabled={run.running || run.clearingCells}
                        onchange={(event) => void run.setKeepCells(event.currentTarget.checked)}
                    />
                    <span>
                        Keep map data for next time
                        <span class="faint">Otherwise deleted after this download.</span>
                    </span>
                </label>
                <button
                    type="button"
                    class="clear-cells"
                    disabled={run.running || run.clearingCells}
                    onclick={() => void run.clearStoredCells(true)}
                >
                    {run.clearingCells ? "Deleting…" : "Delete stored map data"}
                </button>
                {#if run.cellStorageNotice}<span class="faint">{run.cellStorageNotice}</span>{/if}
            </div>

            {#if run.phase === "idle" || run.phase === "cancelled" || run.phase === "error" || run.phase === "done"}
                <!-- No size in the label: the stat band above already leads with it. -->
                <button type="button" class="btn primary" disabled={!run.ready} onclick={() => run.download()}>
                    Download map
                </button>
            {:else}
                <div class="runrow">
                    {#if run.phase !== "finalizing"}
                        <button type="button" class="btn" onclick={() => run.cancel()}>Cancel</button>
                    {/if}
                    {#if run.phase === "downloading" && run.dlProgress}
                        <span class="small muted">
                            downloading cells — {run.dlProgress.completedCells}/{run.dlProgress.totalCells} ·
                            {formatBytes(run.dlProgress.receivedBytes)} of {formatBytes(run.dlProgress.totalBytes)}
                            {#if run.cachedCells > 0}· {run.cachedCells} already on this computer ({formatBytes(
                                    run.cachedBytes,
                                )}){/if}
                        </span>
                    {:else if run.phase === "assembling"}
                        <span class="small muted">
                            assembling — {ASM_PHASE_LABEL[run.asmPhase]} · {Math.round(run.asmFraction * 100)}%{#if run.readMode}
                                · cells {run.readMode}{/if}
                        </span>
                    {:else if run.phase === "saving"}
                        <span class="small muted">
                            {run.sendingToDevice ? "sending the map" : "saving the map"}{#if run.runFileName}
                                — {run.runFileName}{/if}
                        </span>
                    {:else if run.phase === "finalizing"}
                        <span class="small muted">finishing up — removing temporary map data</span>
                    {/if}
                </div>
                <div class="bar">
                    <span
                        style:width={`${run.phase === "downloading" ? dlPct : Math.round(run.asmFraction * 100)}%`}
                        class:assembling={run.phase === "assembling" || run.phase === "saving" || run.phase === "finalizing"}
                    ></span>
                </div>
            {/if}

            {#if run.phase === "done"}
                <div class="done">
                    {#if run.lastRunKind === "device"}
                        <p class="line small">Assembled and sent <span class="mono">{run.runFileName}</span>.</p>
                    {:else}
                        <p class="line small">Downloaded <span class="mono">{run.savedFile?.name}</span>.</p>
                    {/if}
                    {#if run.savedFile}
                        <p class="line faint small mono">{formatBytes(run.savedFile.byteLength)}</p>
                    {/if}
                    {#if run.readMode}
                        <p class="line faint small mono">
                            cells {run.readMode}{#if run.writeMode}
                                · map {run.writeMode}{/if}{#if run.cachedCells > 0}
                                · {run.cachedCells} reused from this computer ({formatBytes(run.cachedBytes)} not
                                downloaded){/if}
                        </p>
                    {/if}
                    {#each run.runWarnings as w, k (k)}
                        <p class="line caution small">{w}</p>
                    {/each}
                </div>
            {:else if run.phase === "error"}
                <p class="line warn small">
                    {#if run.persisted}
                        The map was not completed. Discard the file it left behind: {run.errorMessage}
                    {:else}
                        Nothing was saved: {run.errorMessage}
                    {/if}
                </p>
            {:else if run.phase === "cancelled"}
                <p class="line faint small">
                    {#if run.persisted}
                        Cancelled — discard the incomplete file it left behind.
                    {:else}
                        Cancelled — nothing was saved.
                    {/if}
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

    .cell-storage {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 4px;
        padding: 7px 9px;
        border: 1px solid var(--parchment-3);
        border-radius: 8px;
    }

    .cell-storage label {
        display: flex;
        align-items: flex-start;
        gap: 7px;
        cursor: pointer;
    }

    .cell-storage input {
        margin-top: 3px;
    }

    .clear-cells {
        border: 0;
        padding: 0;
        background: none;
        color: var(--forest);
        text-decoration: underline;
        font: inherit;
        cursor: pointer;
    }

    .clear-cells:disabled,
    .cell-storage label:has(input:disabled) {
        cursor: default;
        opacity: 0.6;
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
</style>
