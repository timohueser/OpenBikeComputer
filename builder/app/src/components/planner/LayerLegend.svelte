<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { Legend } from '../../lib/planner/layers/data-layer';

    /** Swatches, or a continuous scale with labels at its stops. `children` adds items after them. */
    let { legend, label, columns = false, children }: { legend: Legend; label: string; columns?: boolean; children?: Snippet } = $props();
</script>

{#if 'swatches' in legend}
    <ul class="legend" class:columns aria-label={label}>
        {#each legend.swatches as swatch (swatch.label)}<li><span class="swatch" class:hatch={swatch.hatch} style:--swatch={swatch.color}></span>{swatch.label}</li>{/each}
        {@render children?.()}
    </ul>
{:else}
    <div class="legend scale" role="img" aria-label={`${label}: ${legend.scale.map(stop => stop.label).filter(Boolean).join(', ')}`}>
        <div class="ramp">
            <span class="bar" style:background={`linear-gradient(to right, ${legend.scale.map(stop => stop.color).join(', ')})`}></span>
            <ol aria-hidden="true">{#each legend.scale as stop, i (i)}<li>{stop.label}</li>{/each}</ol>
        </div>
        {#if children}<ul>{@render children()}</ul>{/if}
    </div>
{/if}

<style>
    .legend { display: flex; flex-wrap: wrap; gap: 4px 12px; margin: 0; padding: 0; list-style: none; font-size: 12px; color: var(--ink-soft); font-variant-numeric: tabular-nums; }
    .legend.columns { display: grid; grid-template-columns: 1fr 1fr; column-gap: 8px; }
    .legend.columns li { min-height: 22px; }
    ul { margin: 0; padding: 0; list-style: none; }
    .legend :global(li) { display: flex; align-items: center; gap: 6px; }
    .swatch { flex: 0 0 14px; height: 8px; border-radius: 2px; background: var(--swatch); box-shadow: inset 0 0 0 1px var(--line-strong); }
    .columns .swatch { height: 10px; }
    .hatch { background: repeating-linear-gradient(135deg, transparent 0 2px, var(--swatch) 2px 3.5px); }
    .scale { align-items: flex-start; }
    .ramp { flex: 0 1 180px; min-width: 120px; }
    .bar { display: block; height: 8px; border-radius: 2px; box-shadow: inset 0 0 0 1px var(--line-strong); }
    .ramp ol { display: flex; justify-content: space-between; margin: 2px 0 0; padding: 0; list-style: none; font-size: 11px; line-height: 14px; }
</style>
