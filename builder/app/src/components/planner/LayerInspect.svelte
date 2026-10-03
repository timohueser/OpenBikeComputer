<script lang="ts">
    import type { Snippet } from 'svelte';
    import SeasonGrid from './SeasonGrid.svelte';
    import type { Inspection } from '../../lib/planner/layers/data-layer';

    let { title, detail = '', inspection, source, note = '', children }: { title: string; detail?: string; inspection: Inspection | null; source: string; note?: string; children?: Snippet } = $props();
</script>

<div class="inspect">
    <p class="title"><strong>{title}</strong>{#if detail}<span>{detail}</span>{/if}</p>
    {#if inspection}
        <p class="headline">{inspection.headline}</p>
        <SeasonGrid grid={inspection.grid} label={`${inspection.headline}. One row per year, newest at the bottom.`} />
        <ul class="legend">
            {#each inspection.grid.swatches.slice(1) as swatch (swatch.label)}<li><span class="swatch" class:hatch={swatch.hatch} style:--swatch={swatch.color}></span>{swatch.label}</li>{/each}
            <li><span class="line"></span>Your date</li>
        </ul>
    {:else}
        <p class="headline">Loading the years at this point…</p>
    {/if}
    {#if note}<p class="note">{note}</p>{/if}
    {#if source}<p class="source">{source}</p>{/if}
    {@render children?.()}
</div>

<style>
    .inspect { width: 340px; max-width: 100%; padding: 12px 14px; font-size: 13px; color: var(--ink); }
    p { margin: 0; }
    .title { display: flex; align-items: baseline; gap: 8px; margin-bottom: 2px; }
    .title strong { font-size: 14px; font-weight: 600; }
    .title span, .source, .legend { color: var(--ink-soft); font-size: 12px; font-variant-numeric: tabular-nums; }
    .headline { margin-bottom: 10px; }
    .legend { display: flex; flex-wrap: wrap; gap: 4px 12px; margin: 4px 0 0; padding: 0; list-style: none; }
    .legend li { display: flex; align-items: center; gap: 6px; }
    .swatch { width: 14px; height: 8px; border-radius: 2px; background: var(--swatch); }
    .swatch.hatch { background: repeating-linear-gradient(135deg, transparent 0 2px, var(--swatch) 2px 3.5px); box-shadow: inset 0 0 0 1px var(--swatch); }
    .line { width: 2px; height: 12px; background: var(--ink); }
    .source, .note { margin-top: 8px; }
    .note { font-size: 12px; }
</style>
