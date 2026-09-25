<script lang="ts">
    import type { CoverageStore } from "../../lib/coverage/store.svelte";
    import { detailBandId } from "../../lib/coverage/shape";
    import { formatBytes } from "../../lib/format";

    let { store }: { store: CoverageStore } = $props();

    const ledger = $derived(store.ledger);
    const hasParts = $derived(store.selection.parts.length > 0);
    const holeCount = $derived(store.holeCells().length);
    const hasDrawnPartialCoverage = $derived.by(() => {
        if (!store.resolution) return false;
        const detailBand = detailBandId(store.catalog);
        const partial = new Set(store.partialDetailCells());
        return store.resolution.parts.some(({ part, cellsByBand }) =>
            part.kind !== "region" && (cellsByBand.get(detailBand) ?? []).some((id) => partial.has(id)),
        );
    });
    const partialHatchCount = $derived(store.partialHatchCells().length);
</script>

<div class="ledger">
    <h4>Map summary</h4>

    {#if store.indexError}
        <p class="small error">
            Couldn't load map coverage: {store.indexError}
            <button type="button" class="retry" onclick={() => store.reloadIndices()}>retry</button>
        </p>
    {:else if store.resolutionError}
        <p class="small error">This selection can't be built: {store.resolutionError}</p>
    {:else if !ledger}
        <p class="small muted">Loading map coverage…</p>
    {:else if !hasParts}
        <p class="small muted">Choose coverage on the map.</p>
    {:else}
        {#if ledger.isFinal}
            <p class="mono total">
                {formatBytes(ledger.totalBytes)} <span class="small muted">estimated total{ledger.terrain ? ", including elevation" : ""}</span>
            </p>
        {:else}
            <p class="mono total faint">Calculating…</p>
        {/if}

        {#if holeCount > 0}
            <button type="button" class="warnline small" onclick={() => store.focusWarnings("hole")}>
                Map data is missing in some selected areas. <span>Show gaps on map</span>
            </button>
        {/if}
        {#if partialHatchCount > 0}
            <button type="button" class="warnline small" onclick={() => store.focusWarnings("partial")}>
                Street detail may stop near these gaps. <span>Show affected edges</span>
            </button>
        {:else if hasDrawnPartialCoverage}
            <p class="warnline small">
                Street detail may be incomplete at the edge of the available map data.
                Choose a listed region for its published coverage.
            </p>
        {/if}

        {#if ledger.terrain}
            {#if ledger.terrain.missingCount > 0}
                <p class="small error terrain">
                    Elevation data is missing in some selected areas. Climbs there read as flat.
                </p>
            {/if}
            <p class="small faint attribution">{ledger.terrain.attribution}</p>
            {#each ledger.terrain.references as reference (reference.key)}
                <p class="small faint attribution">
                    Summit heights from {reference.product}: {reference.attribution} ({reference.licence})
                </p>
            {/each}
        {/if}

        {#if store.catalog.source}
            <p class="small faint attribution">
                {store.catalog.source.attribution} · <a
                    href={store.catalog.source.license_url}
                    target="_blank"
                    rel="noreferrer">{store.catalog.source.license}</a>
            </p>
        {/if}

        <p class="small faint fit">
            Downloads as one map file. Card space is checked before sending.
        </p>
    {/if}
</div>

<style>
    .ledger {
        border-top: 1px solid var(--line);
        padding-top: 12px;
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

    .warnline {
        text-align: left;
        background: none;
        border: none;
        color: var(--coral);
        padding: 0;
        line-height: 1.4;
        text-decoration: none;
    }

    .warnline span,
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
        font-size: 11px;
    }
</style>
