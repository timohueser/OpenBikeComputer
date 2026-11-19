<script lang="ts">
    import { distanceIn, heightIn, heightUnit } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';
    let { distance, ascent, descent, hours, walking = false }: { distance: number; ascent: number | null; descent: number | null; walking?: boolean; hours: number | null } = $props();
    const minutes = $derived(hours === null ? null : Math.round(hours * 60));
    const height = (metres: number | null) => metres === null ? '—' : Math.round(heightIn(metres));
</script>

<dl class="stats">
    <div><dt>Distance</dt><dd>{distanceIn(distance * 1000).toFixed(1)}<small>{settings.units}</small></dd></div>
    <div><dt>Ascent</dt><dd>{height(ascent)}<small>{heightUnit()}</small></dd></div>
    <div><dt>Descent</dt><dd>{height(descent)}<small>{heightUnit()}</small></dd></div>
    <div><dt>{walking ? 'Walking time' : 'Riding time'}</dt><dd>{minutes === null ? '—' : minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)}h ${minutes % 60}m`}</dd></div>
</dl>

<style>
    .stats {
        display: flex;
        flex-wrap: wrap;
        justify-content: space-between;
        gap: 8px 12px;
        margin: 0;
        padding: 10px 0;
    }
    .stats div { display: flex; flex-direction: column-reverse; }
    dt { font-size: 12px; color: var(--ink-soft); }
    dd { margin: 0; font: 700 17px var(--sans); font-variant-numeric: tabular-nums; white-space: nowrap; }
    small { margin-left: 3px; font: 400 12px var(--sans); color: var(--ink-soft); }
</style>
