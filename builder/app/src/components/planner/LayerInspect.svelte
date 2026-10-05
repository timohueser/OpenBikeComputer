<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import LayerLegend from './LayerLegend.svelte';
    import SeasonGrid from './SeasonGrid.svelte';
    import type { Chart } from '../../lib/planner/layers/data-layer';

    /** A data layer's years at one point: the strip popover, or with `compact` the map callout. */
    let { chart, date, title = '', detail = '', source = '', icon = 'pin', error = '', compact = false }: {
        chart: Chart | null;
        date: string;
        title?: string;
        detail?: string;
        source?: string;
        icon?: string;
        error?: string;
        compact?: boolean;
    } = $props();
</script>

<section class="inspect" class:compact aria-label="Layer at this place">
    {#if compact}
        <p class="headline"><Icon name={icon} size={15} />{chart?.headline ?? (error || 'Loading layer data…')}</p>
    {:else}
        <p class="title"><strong>{title}</strong>{#if detail}<span>{detail}</span>{/if}</p>
        <p class="headline">{chart?.headline ?? (error || 'Loading layer data at this point…')}</p>
    {/if}
    {#if chart}
        {#each chart.grids as grid, g (g)}
            {#if grid.label}<p class="grid-label">{grid.label}</p>{/if}
            <SeasonGrid {grid} {date} rowHeight={compact ? (chart.grids.length > 1 ? 3 : 4) : 8} label={`${grid.label || chart.headline}. One row per year, newest at the bottom.`} />
            {#if !compact && (grid.legend || g === chart.grids.length - 1)}
                <LayerLegend legend={grid.legend ?? { swatches: [] }} label={`${grid.label || 'Chart'} colours`}>
                    {#if g === chart.grids.length - 1}<li><span class="line"></span>Your date</li>{/if}
                </LayerLegend>
            {/if}
        {/each}
        {#if chart.extra}<chart.extra.component {...chart.extra.props} />{/if}
        {#if chart.note}<p class="note">{chart.note}</p>{/if}
    {/if}
    {#if source && !compact}<p class="source">{source}</p>{/if}
</section>

<style>
    .inspect { width: 340px; max-width: 100%; padding: 12px 14px; font-size: 13px; color: var(--ink); }
    .compact { width: auto; margin-top: 14px; padding: 12px 0 0; border-top: 1px solid var(--line); font-size: inherit; }
    p { margin: 0; }
    .title { display: flex; align-items: baseline; gap: 8px; margin-bottom: 2px; }
    .title strong { font-size: 14px; font-weight: 600; }
    .title span, .source { color: var(--ink-soft); font-size: 12px; font-variant-numeric: tabular-nums; }
    .headline { margin-bottom: 10px; }
    .compact .headline { display: flex; align-items: center; gap: 6px; margin-bottom: 8px; }
    .compact .headline :global(svg) { flex: none; color: var(--ink-soft); }
    .grid-label { margin: 6px 0 4px; font-size: 12px; font-weight: 600; color: var(--ink-soft); }
    .inspect :global(.legend) { margin-top: 4px; }
    .line { width: 2px; height: 12px; background: var(--ink); }
    .source, .note { margin-top: 8px; }
    .note { font-size: 12px; }
    .compact .note { margin-top: 4px; color: var(--ink-soft); }
</style>
