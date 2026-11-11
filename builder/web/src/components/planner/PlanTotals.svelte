<script lang="ts">
    import Icon from './PlannerIcon.svelte';

    /** The totals of the whole plan as one line of strong numbers; its labels are for a screen reader only. */
    let { distance, ascent, descent, hours, walking = false }: { distance: number; ascent: number | null; descent: number | null; walking?: boolean; hours: number | null } = $props();
    const minutes = $derived(hours === null ? null : Math.round(hours * 60));
</script>

<!-- An unknown figure shows its icon and a dash, without a unit. -->
<dl class="plan-totals" aria-label="Whole plan">
    <div><dt>Distance</dt><dd><Icon name="route" size={16} />{distance.toFixed(1)}<small>{' '}km</small></dd></div>
    <div><dt>Ascent</dt><dd><Icon name="ascent" size={16} />{#if ascent === null}—{:else}{ascent}<small>{' '}m</small>{/if}</dd></div>
    <div><dt>Descent</dt><dd><Icon name="descent" size={16} />{#if descent === null}—{:else}{descent}<small>{' '}m</small>{/if}</dd></div>
    <div><dt>{walking ? 'Walking time' : 'Riding time'}</dt><dd><Icon name="clock" size={16} />{#if minutes === null}—{:else if minutes < 60}{minutes}<small>{' '}min</small>{:else}{Math.floor(minutes / 60)}<small>{' '}h{' '}</small>{minutes % 60}<small>{' '}m</small>{/if}</dd></div>
</dl>

<style>
    .plan-totals { display: flex; align-items: center; gap: 14px; margin: 0; font: 700 16px/16px var(--mono); font-variant-numeric: tabular-nums; color: var(--ink); white-space: nowrap; }
    div + div { padding-left: 14px; border-left: 1px solid var(--line-strong); }
    /* In a narrow drawer, closer groups keep the line beside the chevron, or at least whole. */
    @container (max-width: 460px) {
        .plan-totals { gap: 8px; }
        div + div { padding-left: 8px; }
    }
    dt { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
    dd { margin: 0; }
    dd :global(svg) { margin-right: 5px; vertical-align: top; color: var(--ink-soft); }
    /* The spaces sit inside the units, so a space is the narrow gap of the unit font and a screen reader still reads "4 h 41 m". A plain space there would be trimmed. */
    small { font: 500 12px var(--sans); color: var(--ink-soft); }
    /* The hours unit: the minutes stand a little further off than a unit from its number. */
    small:not(:last-child) { margin-right: 2px; }
</style>
