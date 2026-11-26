<script lang="ts" module>
    import type { Legend } from '../../lib/planner/layers/data-layer';

    /** The legend of a layer that is on. A note stands in the place of the swatches, such as an error. */
    export type MapLegend = { label: string } & ({ legend: Legend } | { note: string });
</script>

<script lang="ts">
    import LayerLegend from './LayerLegend.svelte';

    /** One pill for each legend, from the top. */
    let { legends }: { legends: MapLegend[] } = $props();
</script>

{#each legends as item (item.label)}
    <div class="pill" role="status">
        <strong>{item.label}</strong>
        {#if 'legend' in item}<LayerLegend legend={item.legend} label={`${item.label} colours`} />{:else}{item.note}{/if}
    </div>
{/each}

<style>
    .pill { justify-self: start; display: flex; flex-wrap: wrap; align-items: center; gap: 4px 12px; padding: 7px 12px; border-radius: 8px; background: var(--panel); box-shadow: var(--planner-shadow); font-size: 12px; color: var(--ink-soft); }
    strong { font-weight: 600; color: var(--ink); }
</style>
