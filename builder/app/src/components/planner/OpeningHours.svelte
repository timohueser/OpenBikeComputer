<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import { openingHoursRows } from '../../lib/planner/search/opening-hours';
    let { value, compact = false }: { value?: string; compact?: boolean } = $props();
    const rows = $derived(value ? openingHoursRows(value) : null);
    const visible = $derived(compact ? rows?.slice(0, 2) : rows);
</script>
<span class="hours" class:compact>
    <span class="heading"><Icon name="clock" size={14} />Opening hours</span>
    {#if rows}
        <span class="schedule">
            {#each visible ?? [] as row}
                <span class="hours-row"><span class="days">{row.days}</span><span class="periods">{#each row.periods as period}<span class:closed={period === 'Closed'}>{period}</span>{/each}</span></span>
            {/each}
        </span>
        {#if compact && rows.length > 2}<span class="more">+{rows.length - 2} more {rows.length === 3 ? 'schedule' : 'schedules'}</span>{/if}
    {:else if value}
        {#if compact}<span class="unknown">View detailed hours</span>{:else}<span class="raw">{value}</span><span class="unknown">Hours as listed in OpenStreetMap</span>{/if}
    {:else}<span class="unknown">No opening hours listed</span>{/if}
</span>
<style>
    .hours { display: block; margin: 16px 0; padding: 14px 0; border-block: 1px solid var(--line); }
    .heading { display: flex; align-items: center; gap: 7px; margin-bottom: 10px; color: var(--ink); font: 600 12px var(--sans); }
    .schedule { display: grid; gap: 8px; }
    .hours-row { display: grid; grid-template-columns: minmax(72px, 1fr) auto; gap: 12px; align-items: baseline; font: 400 13px/1.45 var(--sans); }
    .days { color: var(--ink-soft); }
    .periods { display: grid; justify-items: end; font-variant-numeric: tabular-nums; color: var(--ink); }
    .closed { color: var(--ink-soft); }
    .more, .unknown { display: block; color: var(--ink-soft); font: 400 12px/1.45 var(--sans); }
    .more { margin-top: 6px; }
    .raw { display: block; margin-bottom: 8px; white-space: pre-wrap; overflow-wrap: anywhere; font: 400 13px/1.6 var(--sans); }
    .compact { margin: 8px 0 0; padding: 0; border: 0; }
    .compact .heading { margin-bottom: 5px; font-weight: 400; color: var(--ink-soft); }
    .compact .schedule { gap: 3px; }
    .compact .hours-row { grid-template-columns: minmax(60px, 1fr) auto; gap: 8px; font-size: 12px; }
    .compact .periods { justify-items: start; }
</style>
