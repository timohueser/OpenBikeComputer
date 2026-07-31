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
            <p class="mono total">{formatBytes(ledger.totalBytes)} · {ledger.cellCount} cells</p>
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

    .fit {
        line-height: 1.4;
    }
</style>
