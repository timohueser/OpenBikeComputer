<script lang="ts">
    import { distanceIn, heightIn, heightUnit } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';
    import type { Snippet } from 'svelte';
    // PROTOTYPE: `compact` puts the numbers in one row; `days` adds the day count as a fifth number.
    let { distance, ascent, descent, hours, walking = false, compact = false, days }: { distance: number; ascent: number | null; descent: number | null; walking?: boolean; hours: number | null; compact?: boolean; days?: Snippet } = $props();
    const minutes = $derived(hours === null ? null : Math.round(hours * 60));
    const height = (metres: number | null) => metres === null ? '—' : Math.round(heightIn(metres));
</script>

<dl class="stats" class:compact class:five={!!days}>
    <div><dt>Distance</dt><dd>{distanceIn(distance * 1000).toFixed(1)}<small>{settings.units}</small></dd></div>
    <div><dt>Ascent</dt><dd>{height(ascent)}<small>{heightUnit()}</small></dd></div>
    <div><dt>Descent</dt><dd>{height(descent)}<small>{heightUnit()}</small></dd></div>
    <div><dt>{walking ? 'Walking time' : 'Riding time'}</dt><dd>{minutes === null ? '—' : minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)}h ${minutes % 60}m`}</dd></div>
    {#if days}<div class="days"><dt>Riding days</dt><dd>{@render days()}</dd></div>{/if}
</dl>

<style>
    .stats {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 12px 16px;
        margin: 0;
        padding: 12px 0;
    }
    .stats div { display: flex; flex-direction: column-reverse; }
    dt { font-size: 13px; color: var(--ink-soft); }
    dd { margin: 0; font: 700 22px var(--sans); font-variant-numeric: tabular-nums; white-space: nowrap; }
    .compact { grid-template-columns: repeat(4, auto); justify-content: space-between; gap: 8px; padding: 8px 0; }
    .compact dd { font-size: 17px; }
    .compact dt { font-size: 11px; }
    .five { grid-template-columns: repeat(3, minmax(0, 1fr)); }
    .five .days { grid-column: span 2; }
    .five .days dd { font: inherit; }
    small { margin-left: 4px; font: 400 13px var(--sans); color: var(--ink-soft); }
</style>
