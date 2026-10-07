<script lang="ts">
    import { onDestroy } from 'svelte';
    import LayerLegend from './LayerLegend.svelte';
    import Icon from './PlannerIcon.svelte';
    import SeasonGrid from './SeasonGrid.svelte';
    import DateCalendar from './DateCalendar.svelte';
    import Segmented from './Segmented.svelte';
    import { addDays, addMonths, columnDate, dateLabel, type DataLayer, type Grid, type Legend } from '../../lib/planner/layers/data-layer';

    let { date, year, time, legend, variable, onDate }: { date: string; year?: Grid; time?: DataLayer['time']; legend?: Legend; variable: DataLayer['variable']; onDate: (date: string) => void } = $props();

    let draft = $state(540);
    let pending: ReturnType<typeof setTimeout> | undefined;
    onDestroy(() => clearTimeout(pending));
    $effect(() => { if (time) { const [h, m] = time.value.split(':').map(Number); draft = h * 60 + m; } });
    function commit() {
        clearTimeout(pending); pending = undefined;
        if (time) time.value = `${String(Math.floor(draft / 60)).padStart(2, '0')}:${String(draft % 60).padStart(2, '0')}`;
    }
    function preview() { pending ??= setTimeout(commit, 100); }
    let open = $state(false);
    let picker: HTMLElement;
    let toggle: HTMLButtonElement;
    const longDate = $derived(new Date(date).toLocaleDateString('en-GB', { day: 'numeric', month: 'long', timeZone: 'UTC', ...(time ? { year: 'numeric' } : {}) }));

    /** A step from the stepper or the keyboard; the calendar closes, so it never shows a stale month. */
    function step(next: string) {
        open = false;
        onDate(next);
    }
    function shift(days: number) {
        step(addDays(date, days));
    }
    function key(event: KeyboardEvent) {
        const year = date.slice(0, 4);
        if (event.key === 'Home' || event.key === 'End') step(`${year}-${event.key === 'Home' ? '01-01' : '12-31'}`);
        else if (event.key === 'PageUp' || event.key === 'PageDown') step(addMonths(date, event.key === 'PageUp' ? 1 : -1));
        else {
            const days = { ArrowLeft: -1, ArrowDown: -1, ArrowRight: 1, ArrowUp: 1 }[event.key];
            if (!days) return;
            shift(event.shiftKey ? days * 7 : days);
        }
        event.preventDefault();
    }
    function close() {
        open = false;
        toggle.focus();
    }
</script>

<svelte:window onpointerdown={(event) => { if (open && !picker.contains(event.target as Node)) open = false; }} />

<div class="date-bar" role="group" aria-label="Layer date">
    <div class="stepper" bind:this={picker}>
        <button type="button" aria-label="Previous day" onclick={() => shift(-1)}><Icon path="m15 5-7 7 7 7" /></button>
        <button type="button" class="date" bind:this={toggle} aria-haspopup="dialog" aria-expanded={open} aria-label={`Layer date: ${longDate}. Choose a day`}
            title="Choose a day. Arrow keys change the day, Shift the week" onclick={() => open = !open} onkeydown={key}>{time ? longDate : dateLabel(date)}<Icon name="calendar" size={14} /></button>
        <button type="button" aria-label="Next day" onclick={() => shift(1)}><Icon name="chevron" /></button>
        {#if open}
            <div class="popover"><DateCalendar {date} onPick={(day) => { onDate(day); close(); }} onClose={close} /></div>
        {/if}
    </div>
    {#if variable}<div class="variable"><Segmented compact label={variable.label} options={variable.options} value={variable.value} onChange={(value) => variable.value = value} /></div>{/if}
    {#if time}
        <label class="clock"><Icon name="clock" size={16} /><input type="time" aria-label="Sunlight time" value={time.value} step="300" onchange={(event) => { if (event.currentTarget.value) time.value = event.currentTarget.value; }} /></label>
        <div class="time-slider">
            <input type="range" min="0" max="1435" step="5" bind:value={draft} aria-label="Time of day" aria-valuetext={`${String(Math.floor(draft / 60)).padStart(2, '0')}:${String(draft % 60).padStart(2, '0')}`} oninput={preview} onchange={commit} />
            <div class="time-labels"><span>00:00</span><span>{time.timezone || 'Loading local time…'}</span><span>24:00</span></div>
            {#if time.detail}<p class="hint">{time.detail}</p>{/if}
            {#if legend}<LayerLegend {legend} label="Sunlight colours" />{/if}
        </div>
    {/if}
    {#if year}<div class="year">
        <p>{year.label}</p>
        <SeasonGrid grid={year} {date} rowHeight={14} label="Layer date in the year" slider={{ valueText: `${longDate}. ${year.label}`, onKey: key }}
            onPick={(column) => step(columnDate(column, Number(date.slice(0, 4)), year.columns))} />
    </div>{/if}
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
    .stepper { position: relative; display: inline-flex; align-items: center; flex: none; border: 1px solid var(--line-strong); border-radius: 8px; background: var(--panel); }
    .stepper button { display: grid; place-items: center; width: 32px; height: 34px; color: var(--ink); }
    .stepper button:hover { background: var(--parchment-2); }
    .stepper > button:first-child { border-radius: 7px 0 0 7px; }
    .stepper > button:nth-child(3) { border-radius: 0 7px 7px 0; }
    .stepper .date { display: inline-flex; align-items: center; justify-content: center; gap: 6px; width: auto; min-width: 84px; padding: 0 8px; border-inline: 1px solid var(--line); font-weight: 700; font-variant-numeric: tabular-nums; white-space: nowrap; cursor: pointer; }
    .date :global(svg) { color: var(--ink-soft); }
    .date[aria-expanded="true"] { background: var(--parchment-2); }
    .date:focus-visible { outline: 2px solid var(--forest); outline-offset: 2px; }
    .popover { position: absolute; left: 0; bottom: calc(100% + 12px); z-index: 1; }
    .hint { margin: 4px 0 8px; color: var(--ink-soft); font-size: 12px; }
    .clock { display: flex; align-items: center; gap: 6px; font-variant-numeric: tabular-nums; }
    .clock input { width: 98px; color: var(--ink); font: inherit; font-weight: 600; background: var(--panel); border: 1px solid var(--line-strong); border-radius: 6px; padding: 5px 6px; }
    .time-slider { flex: 1 1 100%; min-width: 0; }
    .time-slider input { width: 100%; height: 24px; margin: 0; accent-color: var(--rust); cursor: pointer; }
    .time-slider input:focus-visible, .clock input:focus-visible { outline: 2px solid var(--forest); outline-offset: 2px; }
    .time-labels { display: flex; justify-content: space-between; margin: 0 0 6px; font-size: 11px; color: var(--ink-soft); font-variant-numeric: tabular-nums; }
    .variable { flex: none; }
    /* On a narrow map the year takes its own line under the date and the switch. */
    .year { flex: 1 1 360px; min-width: 0; }
    .year p { margin: 0 0 6px; font-size: 12px; color: var(--ink-soft); }
</style>
