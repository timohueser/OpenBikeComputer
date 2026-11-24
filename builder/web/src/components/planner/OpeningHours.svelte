<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import { openingHoursRows } from '../../lib/planner/search/opening-hours';
    let { value }: { value?: string } = $props();
    const rows = $derived(value ? openingHoursRows(value) : null);
</script>
<span class="hours">
    <span class="heading" class:none={!value}><Icon name="clock" size={14} />{value ? 'Opening hours' : 'No opening hours listed'}</span>
    {#if rows}
        <span class="schedule">
            {#each rows as row}
                <span class="hours-row"><span class="days">{row.days}</span><span class="periods">{#each row.periods as period}<span class:closed={period === 'Closed'}>{period}</span>{/each}</span></span>
            {/each}
        </span>
    {:else if value}
        <span class="raw">{value}</span><span class="unknown">Hours as listed in OpenStreetMap</span>
    {/if}
</span>
<style>
    .hours { display: block; padding-top: 12px; border-top: 1px solid var(--line); }
    .heading { display: flex; align-items: center; gap: 7px; margin-bottom: 6px; color: var(--ink); font: 600 13px var(--sans); }
    .none { margin-bottom: 0; color: var(--ink-soft); font-weight: 400; }
    .schedule { display: grid; gap: 3px; }
    .hours-row { display: grid; grid-template-columns: minmax(72px, 1fr) auto; gap: 12px; align-items: baseline; font: 400 13px/1.45 var(--sans); }
    .days { color: var(--ink-soft); }
    .periods { display: grid; justify-items: end; font-variant-numeric: tabular-nums; color: var(--ink); }
    .closed { color: var(--ink-soft); }
    .unknown { display: block; color: var(--ink-soft); font: 400 12px/1.45 var(--sans); }
    .raw { display: block; margin-bottom: 8px; white-space: pre-wrap; overflow-wrap: anywhere; font: 400 13px/1.6 var(--sans); }
</style>
