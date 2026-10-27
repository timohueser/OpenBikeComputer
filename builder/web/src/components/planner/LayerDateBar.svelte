<script lang="ts">
    import SeasonGrid from './SeasonGrid.svelte';
    import DateStepper, { dayKey } from './DateStepper.svelte';
    import Segmented from './Segmented.svelte';
    import { columnDate, dateLabel, type DataLayer, type Grid } from '../../lib/planner/layers/data-layer';

    let { date, year, variable, onDate }: { date: string; year: Grid; variable: DataLayer['variable']; onDate: (date: string) => void } = $props();

    const longDate = $derived(new Date(date).toLocaleDateString('en-GB', { day: 'numeric', month: 'long', timeZone: 'UTC' }));
</script>

<div class="date-bar" role="group" aria-label="Layer date">
    <DateStepper {date} text={dateLabel(date)} label={`Layer date: ${longDate}. Choose a day`} {onDate} />
    {#if variable}<div class="variable"><Segmented compact label={variable.label} options={variable.options} value={variable.value} onChange={(value) => variable.value = value} /></div>{/if}
    <div class="year">
        <p>{year.label}</p>
        <SeasonGrid grid={year} {date} rowHeight={14} label="Layer date in the year" slider={{ valueText: `${longDate}. ${year.label}`, onKey: (event) => dayKey(event, date, onDate) }}
            onPick={(column) => onDate(columnDate(column, Number(date.slice(0, 4)), year.columns))} />
    </div>
</div>

<style>
    .date-bar {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 8px 16px;
        padding: 10px 16px 6px;
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
    }
    .variable { flex: none; }
    /* On a narrow map the year takes its own line under the date and the switch. */
    .year { flex: 1 1 360px; min-width: 0; }
    .year p { margin: 0 0 6px; font-size: 12px; color: var(--ink-soft); }
</style>
