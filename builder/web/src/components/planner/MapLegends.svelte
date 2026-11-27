<script lang="ts" module>
    import type { Legend } from '../../lib/planner/layers/data-layer';

    /**
     * The legend of a layer that is on. `note` ends the name, and `hint` explains it on hover. `faint` dims the swatches
     * of a layer that the map does not draw at its zoom. A layer with no legend yet shows `text`, such as an error.
     */
    export interface MapLegend { label: string; legend?: Legend; text?: string; note?: string; hint?: string; faint?: boolean }
</script>

<script lang="ts">
    import LayerLegend from './LayerLegend.svelte';

    /** One pill for each legend, from the top. */
    let { legends }: { legends: MapLegend[] } = $props();
</script>

{#each legends as { label, legend, text, note, hint, faint } (label)}
    <div class="pill" class:faint role="status">
        <span class="name"><strong>{label}</strong>{#if note}<span title={hint}>{` · ${note}`}</span>{/if}</span>
        {#if legend}<LayerLegend {legend} label={`${label} colours`} />{:else}{text}{/if}
    </div>
{/each}

<style>
    /* A note never changes the height of a pill: the map fits a route clear of the pills, so a height that follows the
       zoom would move the zoom again. The name has a line of its own, and the swatches wrap under it. */
    .pill { justify-self: start; box-sizing: border-box; max-width: 100%; display: flex; flex-direction: column; align-items: flex-start; gap: 4px 12px; padding: 7px 12px; border-radius: 8px; background: var(--panel); box-shadow: var(--planner-shadow); font-size: 12px; color: var(--ink-soft); }
    .name { max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    /* From this width of the stack of legends, each pill is one line that never wraps. The longest pill with its longest
       note, the steepness with "coarse terrain here", fits with some room for another font. */
    @container (min-width: 760px) {
        .pill { flex-direction: row; align-items: center; white-space: nowrap; }
        .pill > :global(*) { flex: none; }
    }
    strong { font-weight: 600; color: var(--ink); }
    .faint > :global(.legend) { opacity: 0.4; }
    [title] { cursor: help; }
</style>
