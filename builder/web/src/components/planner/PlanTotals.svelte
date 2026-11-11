<script lang="ts">
    /** The totals of the whole plan as one line of strong numbers; its labels are for a screen reader only. */
    let { distance, ascent, descent, hours, walking = false }: { distance: number; ascent: number | null; descent: number | null; walking?: boolean; hours: number | null } = $props();
    const minutes = $derived(hours === null ? null : Math.round(hours * 60));
</script>

<dl class="plan-totals" aria-label="Whole plan">
    <div><dt>Distance</dt><dd>{distance.toFixed(1)}<small>{' '}km</small></dd></div>
    <div><dt>Ascent</dt><dd><small aria-hidden="true">↑</small>{ascent ?? '—'}<small>{' '}m</small></dd></div>
    <div><dt>Descent</dt><dd><small aria-hidden="true">↓</small>{descent ?? '—'}<small>{' '}m</small></dd></div>
    <div><dt>{walking ? 'Walking time' : 'Riding time'}</dt><dd>{#if minutes === null}—{:else if minutes < 60}{minutes}<small>{' '}min</small>{:else}{Math.floor(minutes / 60)}<small>{' '}h{' '}</small>{minutes % 60}<small>{' '}m</small>{/if}</dd></div>
</dl>

<style>
    .plan-totals { display: flex; align-items: baseline; gap: 16px; margin: 0; font: 700 16px var(--mono); font-variant-numeric: tabular-nums; color: var(--ink); white-space: nowrap; }
    dt { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
    dd { display: inline; margin: 0; }
    /* The spaces sit inside the units, so they take the narrow width of the units. A plain space there would be trimmed. */
    small { font: 400 12px var(--mono); color: var(--ink-soft); }
</style>
