<script module lang="ts">
    import { addDays, addMonths } from '../../lib/planner/layers/data-layer';

    /** Arrow keys step a day, with Shift a week; Page Up and Page Down a month; Home and End go to the ends of the year. */
    export function dayKey(event: KeyboardEvent, date: string, onDate: (date: string) => void): void {
        const year = date.slice(0, 4);
        if (event.key === 'Home' || event.key === 'End') onDate(`${year}-${event.key === 'Home' ? '01-01' : '12-31'}`);
        else if (event.key === 'PageUp' || event.key === 'PageDown') onDate(addMonths(date, event.key === 'PageUp' ? 1 : -1));
        else {
            const days = { ArrowLeft: -1, ArrowDown: -1, ArrowRight: 1, ArrowUp: 1 }[event.key];
            if (!days) return;
            onDate(addDays(date, event.shiftKey ? days * 7 : days));
        }
        event.preventDefault();
    }
</script>

<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import DateCalendar from './DateCalendar.svelte';

    /** The previous day, the date that opens the calendar, and the next day. `label` names the date button for a screen reader. */
    let { date, text, label, onDate, below = false }: { date: string; text: string; label: string; onDate: (date: string) => void; below?: boolean } = $props();

    let open = $state(false);
    let picker: HTMLElement;
    let toggle: HTMLButtonElement;
    // The calendar closes at each new date, so it never shows a stale month.
    $effect(() => {
        void date;
        open = false;
    });
    function close() {
        open = false;
        toggle.focus();
    }
</script>

<svelte:window onpointerdown={(event) => { if (open && !picker.contains(event.target as Node)) open = false; }} />

<div class="stepper" bind:this={picker}>
    <button type="button" aria-label="Previous day" onclick={() => onDate(addDays(date, -1))}><Icon path="m15 5-7 7 7 7" /></button>
    <button type="button" class="date" bind:this={toggle} aria-haspopup="dialog" aria-expanded={open} aria-label={label}
        title="Choose a day. Arrow keys change the day, Shift the week" onclick={() => open = !open} onkeydown={(event) => dayKey(event, date, onDate)}>{text}<Icon name="calendar" size={14} /></button>
    <button type="button" aria-label="Next day" onclick={() => onDate(addDays(date, 1))}><Icon name="chevron" /></button>
    {#if open}
        <div class="popover" class:below><DateCalendar {date} onPick={(day) => { onDate(day); close(); }} onClose={close} /></div>
    {/if}
</div>

<style>
    .stepper { position: relative; display: flex; align-items: center; flex: none; border: 1px solid var(--line-strong); border-radius: 8px; background: var(--panel); }
    button { display: grid; place-items: center; width: 32px; height: 34px; padding: 0; border: 0; background: none; color: var(--ink); font: inherit; cursor: pointer; }
    button:hover { background: var(--parchment-2); }
    button:first-child { border-radius: 7px 0 0 7px; }
    button:nth-child(3) { border-radius: 0 7px 7px 0; }
    .date { display: inline-flex; flex: 1; align-items: center; justify-content: center; gap: 6px; width: auto; min-width: 84px; padding: 0 8px; border-inline: 1px solid var(--line); font-weight: 700; font-variant-numeric: tabular-nums; white-space: nowrap; }
    .date :global(svg) { color: var(--ink-soft); }
    .date[aria-expanded="true"] { background: var(--parchment-2); }
    .date:focus-visible { outline: 2px solid var(--forest); outline-offset: 2px; }
    .popover { position: absolute; left: 0; bottom: calc(100% + 12px); z-index: 1; }
    .popover.below { top: calc(100% + 8px); bottom: auto; }
</style>
