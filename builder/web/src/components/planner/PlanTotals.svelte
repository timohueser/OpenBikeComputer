<script lang="ts">
    /** The totals of the whole plan as one quiet line; its labels are for a screen reader only. */
    let { distance, ascent, descent, hours, walking = false }: { distance: number; ascent: number | null; descent: number | null; walking?: boolean; hours: number | null } = $props();
    const minutes = $derived(hours === null ? null : Math.round(hours * 60));
</script>

<dl class="plan-totals" aria-label="Whole plan">
    <div><dt>Distance</dt><dd>{distance.toFixed(1)} <small>km</small></dd></div>
    <div><dt>Ascent</dt><dd><small aria-hidden="true">↑</small> {ascent ?? '—'} <small>m</small></dd></div>
    <div><dt>Descent</dt><dd><small aria-hidden="true">↓</small> {descent ?? '—'} <small>m</small></dd></div>
    <div><dt>{walking ? 'Walking time' : 'Riding time'}</dt><dd>{#if minutes === null}—{:else if minutes < 60}{minutes} <small>min</small>{:else}{Math.floor(minutes / 60)} <small>h</small> {minutes % 60} <small>m</small>{/if}</dd></div>
</dl>

<style>
    .plan-totals { display: flex; gap: 8px; margin: 0; font: 13px var(--mono); font-variant-numeric: tabular-nums; color: var(--ink); white-space: nowrap; }
    /* The empty alternative text keeps the separator out of the screen reader. */
    div + div::before { content: "·" / ""; margin-right: 8px; color: var(--ink-soft); }
    dt { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
    dd { display: inline; margin: 0; }
    small { font-size: inherit; color: var(--ink-soft); }
</style>
