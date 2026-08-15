<script lang="ts">
    // The Map summary ledger (#1038, §8 U4): the always-visible card that keeps
    // score — total bytes, cells, and every warning the selection has earned.
    //
    // Three disciplines from the ledger module, honoured rather than restated:
    // the total is summed real cell bytes and is only *printed* once `isFinal`
    // (a pending region prices as 0 B with a straight face otherwise); the
    // refuse/warn verdict arrives with both figures and the navigation graph
    // named, and is shown verbatim; and partial cells in the coarse context
    // band never appear here at all.
    //
    // The fits-on-card meter (§9/D4: SD free space, no user-visible file-size
    // limit) needs a number only a connected card can give, and connecting is
    // step 4's moment — so until then the card says where that check happens
    // instead of drawing a meter against a guess.

    import type { CoverageStore } from "../../lib/coverage/store.svelte";
    import { formatBytes } from "../../lib/format";

    let { store }: { store: CoverageStore } = $props();

    const ledger = $derived(store.ledger);
    const hasParts = $derived(store.selection.parts.length > 0);
    // Holes from every band, matching the hatched squares one for one (#1041
    // A5 — `store.holeCells` explains the dedup). The partial sentence keeps
    // the detail band's full count while the map hatches only the
    // hole-adjacent subset (#1041 A9), so the line is a zoom target exactly
    // when there is hatch to zoom to.
    const holeCount = $derived(store.holeCells().length);
    const partialCount = $derived(store.partialDetailCells().length);
    const partialHatchCount = $derived(store.partialHatchCells().length);
</script>

<div class="ledger">
    <h4>Map summary</h4>

    {#if store.indexError}
        <p class="small error">
            Couldn't load the cell catalog: {store.indexError}
            <button type="button" class="retry" onclick={() => store.reloadIndices()}>retry</button>
        </p>
    {:else if store.resolutionError}
        <p class="small error">This selection can't be built: {store.resolutionError}</p>
    {:else if !ledger}
        <p class="small muted">Loading the cell catalog…</p>
    {:else if !hasParts}
        <p class="small muted">Nothing selected yet — the summary keeps score as you add parts.</p>
    {:else}
        {#if ledger.isFinal}
            <p class="mono total">
                {formatBytes(ledger.totalBytes)} · {ledger.cellCount}
                {ledger.cellCount === 1 ? "cell" : "cells"}
            </p>
        {:else}
            <p class="mono total faint">pricing…</p>
        {/if}

        {#if ledger.verdict.kind === "refuse"}
            <p class="small verdict refuse">{ledger.verdict.message}</p>
        {:else if ledger.verdict.kind === "warn"}
            <p class="small verdict warn">{ledger.verdict.message}</p>
        {/if}

        {#if holeCount > 0}
            <button type="button" class="warnline small" onclick={() => store.focusWarnings("hole")}>
                ⚠ {holeCount}
                {holeCount === 1 ? "cell" : "cells"} not baked yet — the map will have holes there
            </button>
        {/if}
        {#if partialCount > 0}
            {#if partialHatchCount > 0}
                <button type="button" class="warnline small" onclick={() => store.focusWarnings("partial")}>
                    ⚠ {partialCount}
                    {partialCount === 1 ? "cell is" : "cells are"} only partly baked — detail may stop at
                    the extract's edge
                </button>
            {:else}
                <!-- Same sentence, not a button: nothing is hatched (no partial
                     cell abuts a hole, #1041 A9), so there is nothing on the
                     map for a click to fly to. -->
                <p class="warnline small">
                    ⚠ {partialCount}
                    {partialCount === 1 ? "cell is" : "cells are"} only partly baked — detail may stop at
                    the extract's edge
                </p>
            {/if}
        {/if}

        <!-- Elevation (EL4). One line, no toggle: terrain is ~5 % of a download
             and a switch would be a decision a rider should not have to make.
             The size is stated separately from the map's because OBCC §13.3
             requires it — the two are separate prices — and the credit is the
             catalog's own string, which §13.5 makes a MUST rather than a
             courtesy: a dataset change must carry its own notice with it. -->
        {#if ledger.terrain}
            <p class="small faint terrain">
                Includes {formatBytes(ledger.terrain.bytes)} of elevation data.
                {#if ledger.terrain.missingCount > 0}
                    {ledger.terrain.missingCount}
                    {ledger.terrain.missingCount === 1 ? "square has" : "squares have"} no elevation coverage;
                    climbs there read as flat.
                {/if}
            </p>
            <p class="small faint attribution">{ledger.terrain.attribution}</p>
        {/if}

        <!-- The map data's own credit (§3.1) — the catalog's string, the same
             take-it-from-the-document rule as the terrain line above. The map
             this card prices is a derivative database of OSM, and the licence
             is part of what a rider downloads. -->
        {#if store.catalog.source}
            <p class="small faint attribution">
                {store.catalog.source.attribution} · <a
                    href={store.catalog.source.license_url}
                    target="_blank"
                    rel="noreferrer">{store.catalog.source.license}</a>
            </p>
        {/if}

        <p class="small faint fit">
            Whether it fits your SD card is checked against the connected card in step 4 — maps of any
            size arrive as a set of files.
        </p>
    {/if}
</div>

<style>
    .ledger {
        background: var(--parchment);
        border: 1px solid var(--parchment-3);
        border-radius: 10px;
        padding: 11px 13px;
        display: flex;
        flex-direction: column;
        gap: 7px;
    }

    h4 {
        font-family: var(--serif);
        font-size: 14px;
        margin: 0;
    }

    p {
        margin: 0;
    }

    .total {
        font-size: 14px;
    }

    .error {
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

    .verdict {
        border-radius: 8px;
        padding: 7px 10px;
        line-height: 1.45;
    }

    .verdict.warn {
        background: rgba(227, 173, 51, 0.18);
        border: 1px solid var(--amber);
    }

    .verdict.refuse {
        background: rgba(207, 106, 42, 0.12);
        border: 1px solid var(--coral);
        color: var(--coral);
    }

    .warnline {
        text-align: left;
        background: none;
        border: none;
        color: var(--coral);
        padding: 0;
        line-height: 1.4;
        text-decoration: none;
    }

    button.warnline:hover {
        text-decoration: underline;
    }

    .fit,
    .terrain {
        line-height: 1.4;
    }

    /* The source credit is a licence obligation, not a caption — small, but
       never hidden, never truncated, and never a tooltip. */
    .attribution {
        line-height: 1.35;
        font-size: 10px;
        opacity: 0.75;
    }
</style>
