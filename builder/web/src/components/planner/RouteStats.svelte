<script lang="ts">
    import type { Bike } from '../../lib/library/records';
    import { plannedSeconds } from '../../lib/settings/format';

    /** `hours` is the routing time; the stats show it at the pace of the rider. */
    let { distance, ascent, descent, hours, bike }: { distance: number; ascent: number | null; descent: number | null; bike: Bike; hours: number | null } = $props();
    const minutes = $derived(hours === null ? null : Math.round(plannedSeconds(hours * 3600, bike) / 60));
</script>

<dl class="stats">
    <div><dt>Distance</dt><dd>{distance.toFixed(1)}<small>km</small></dd></div>
    <div><dt>Ascent</dt><dd>{ascent ?? '—'}<small>m</small></dd></div>
    <div><dt>Descent</dt><dd>{descent ?? '—'}<small>m</small></dd></div>
    <div><dt>{bike === 'hiking' ? 'Walking time' : 'Riding time'}</dt><dd>{minutes === null ? '—' : minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)}h ${minutes % 60}m`}</dd></div>
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
    small { margin-left: 4px; font: 400 13px var(--sans); color: var(--ink-soft); }
</style>
