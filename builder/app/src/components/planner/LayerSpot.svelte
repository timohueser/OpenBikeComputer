<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import SeasonGrid from './SeasonGrid.svelte';
    import type { Inspection } from '../../lib/planner/layers/data-layer';

    /** A data layer's years at one map spot, compact enough for the map callout. */
    let { inspection, error = '', note = '' }: { inspection: Inspection | null; error?: string; note?: string } = $props();
</script>

<section class="spot" aria-label="Past years here">
    <p class="headline"><Icon name="snow" size={15} />{inspection?.headline ?? (error || 'Loading past years…')}</p>
    {#if inspection}<SeasonGrid grid={inspection.grid} rowHeight={4} label={`${inspection.headline}. One row per year, newest at the bottom.`} />{/if}
    {#if note}<p class="note">{note}</p>{/if}
</section>

<style>
    .spot { margin-top: 14px; padding-top: 12px; border-top: 1px solid var(--line); }
    p { margin: 0; }
    .headline { display: flex; align-items: center; gap: 6px; margin-bottom: 8px; color: var(--ink); }
    .headline :global(svg) { flex: none; color: var(--ink-soft); }
    .note { margin-top: 4px; font-size: 12px; color: var(--ink-soft); }
</style>
