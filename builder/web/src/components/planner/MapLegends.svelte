<script lang="ts" module>
    import type { Legend } from '../../lib/planner/layers/data-layer';

    /**
     * The legend of a layer that is on. `note` ends the name, and `hint` explains it on hover. `faint` dims the swatches
     * of a layer that the map does not draw at its zoom. A note and faint swatches keep the height of the pill: the map
     * fits a route clear of the pills, so a height that follows the zoom would move the zoom again. A layer with no
     * legend yet shows `text`, such as an error.
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
        <span><strong>{label}</strong>{#if note}<span title={hint}>{` · ${note}`}</span>{/if}</span>
        {#if legend}<LayerLegend {legend} label={`${label} colours`} />{:else}{text}{/if}
    </div>
{/each}

<style>
    .pill { justify-self: start; display: flex; flex-wrap: wrap; align-items: center; gap: 4px 12px; padding: 7px 12px; border-radius: 8px; background: var(--panel); box-shadow: var(--planner-shadow); font-size: 12px; color: var(--ink-soft); }
    strong { font-weight: 600; color: var(--ink); }
    .faint > :global(.legend) { opacity: 0.4; }
</style>
