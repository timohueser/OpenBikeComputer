<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { Legend } from '../../lib/planner/layers/data-layer';

    /** Swatches, or a continuous scale with labels at its stops. `children` adds items after them. */
    let { legend, label, children }: { legend: Legend; label: string; children?: Snippet } = $props();
</script>

{#snippet marks()}
    {#each legend.marks ?? [] as mark (mark.label)}
        <li><svg class="mark" viewBox="0 0 24 24" aria-hidden="true"><path d={mark.path} stroke-width={mark.width} /></svg>{mark.label}</li>
    {/each}
{/snippet}

{#if 'swatches' in legend}
    <ul class="legend" aria-label={label}>
        {#each legend.swatches as swatch (swatch.label)}<li><span class="swatch" class:hatch={swatch.hatch} style:--swatch={swatch.color}></span>{swatch.label}</li>{/each}
        {@render marks()}
        {@render children?.()}
    </ul>
{:else}
    <div class="legend scale" role="img" aria-label={`${label}: ${legend.scale.map(stop => stop.label).filter(Boolean).join(', ')}`}>
        <div class="ramp">
            <span class="bar" style:background={`linear-gradient(to right, ${legend.scale.map(stop => stop.color).join(', ')})`}></span>
            <ol aria-hidden="true">{#each legend.scale as stop, i (i)}<li style:left={`${(100 * i) / (legend.scale.length - 1)}%`}>{stop.label}</li>{/each}</ol>
        </div>
        {#if children || legend.marks}<ul>{@render marks()}{@render children?.()}</ul>{/if}
    </div>
{/if}

<style>
    .legend { display: flex; flex-wrap: wrap; gap: 4px 12px; margin: 0; padding: 0; list-style: none; font-size: 12px; color: var(--ink-soft); font-variant-numeric: tabular-nums; }
    ul { margin: 0; padding: 0; list-style: none; }
    .legend :global(li) { display: flex; align-items: center; gap: 6px; }
    .swatch { flex: 0 0 14px; height: 8px; border-radius: 2px; background: var(--swatch); box-shadow: inset 0 0 0 1px var(--line-strong); }
    .hatch { background: repeating-linear-gradient(135deg, transparent 0 2px, var(--swatch) 2px 3.5px); }
    .mark { flex: none; width: 20px; height: 20px; margin: -6px -3px; fill: none; stroke: var(--ink); stroke-linecap: round; stroke-linejoin: round; }
    .scale { align-items: flex-start; }
    .ramp { flex: 0 1 auto; width: 260px; min-width: 120px; }
    .bar { display: block; height: 8px; border-radius: 2px; box-shadow: inset 0 0 0 1px var(--line-strong); }
    /* Each label stands under its stop; the end labels stay inside the ramp. */
    .ramp ol { position: relative; height: 14px; margin: 2px 0 0; padding: 0; list-style: none; font-size: 11px; line-height: 14px; }
    .ramp ol li { position: absolute; translate: -50%; white-space: nowrap; }
    .ramp ol li:first-child { translate: none; }
    .ramp ol li:last-child { translate: -100%; }
</style>
