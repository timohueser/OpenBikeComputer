<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import SeasonGrid from './SeasonGrid.svelte';
    import { columnDate, dateLabel, type LineStats } from '../../lib/planner/layers/data-layer';

    let { date, year, onDate }: { date: string; year: LineStats['year'] | null; onDate: (date: string) => void } = $props();

    function step(days: number) {
        const [y, m, d] = date.split('-').map(Number);
        onDate(new Date(Date.UTC(y, m - 1, d + days)).toISOString().slice(0, 10));
    }
    function key(event: KeyboardEvent) {
        const days = { ArrowLeft: -1, ArrowDown: -1, ArrowRight: 1, ArrowUp: 1 }[event.key];
        if (!days) return;
        event.preventDefault();
        step(event.shiftKey ? days * 7 : days);
    }
</script>

<div class="date-bar" role="group" aria-label="Layer date">
    <div class="stepper">
        <button type="button" aria-label="Previous day" onclick={() => step(-1)}><Icon path="m15 5-7 7 7 7" /></button>
        <span class="date" role="spinbutton" tabindex="0" aria-label="Layer date" aria-valuetext={new Date(date).toLocaleDateString('en-GB', { day: 'numeric', month: 'long', timeZone: 'UTC' })}
            title="Arrow keys change the day, Shift the week" onkeydown={key}>{dateLabel(date)}</span>
        <button type="button" aria-label="Next day" onclick={() => step(1)}><Icon name="chevron" /></button>
    </div>
    <div class="year">
        {#if year}
            <p>{year.label}</p>
            <SeasonGrid grid={year.grid} rowHeight={14} label={`${year.label}. Click a day to show it.`} onPick={(column) => onDate(columnDate(column, Number(date.slice(0, 4))))} />
        {:else}
            <p>Plan a route to see when the whole route is snow-free.</p>
        {/if}
    </div>
</div>

<style>
    .date-bar {
        display: flex;
        align-items: center;
        gap: 16px;
        padding: 10px 16px 6px;
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
    }
    .stepper { display: inline-flex; align-items: center; flex: none; border: 1px solid var(--line-strong); border-radius: 8px; background: var(--panel); }
    .stepper button { display: grid; place-items: center; width: 32px; height: 34px; color: var(--ink); }
    .stepper button:hover { background: var(--parchment-2); }
    .date { min-width: 64px; padding: 0 4px; border-inline: 1px solid var(--line); text-align: center; font-weight: 700; line-height: 34px; font-variant-numeric: tabular-nums; white-space: nowrap; }
    .date:focus-visible { outline: 2px solid var(--forest); outline-offset: 2px; }
    .year { flex: 1; min-width: 0; }
    .year p { margin: 0 0 6px; font-size: 12px; color: var(--ink-soft); }
</style>
