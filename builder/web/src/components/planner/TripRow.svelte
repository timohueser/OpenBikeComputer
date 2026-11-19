<!-- The trip bar: one row of equal cells for the day count, the dates and, for a trip of two or more days, the places
     to sleep. A grey value is unset or a guess; a value that the rider set is ink. -->
<script lang="ts">
    import DayCount from './DayCount.svelte';
    import StayChip from './StayChip.svelte';
    import { allStay, multiDay, type Stay, type Trip } from '../../lib/planner/editor';
    import { addDays, weekdayLabel } from '../../lib/planner/layers/data-layer';
    import type { RoutingLine } from '../../lib/planner/routing';
    import { stayOptions as kinds } from '../../lib/planner/search/presentation';

    let { trip, line, onChange, onDays }: {
        trip: Trip; line: RoutingLine | undefined;
        onChange: (next: Trip, description: string) => void;
        /** A change of the day count, which can change the type of the route. */
        onDays: (next: Trip, description: string) => void;
    } = $props();

    let dating = $state(false);
    let root = $state<HTMLDivElement>();
    const multi = $derived(multiDay(trip));
    const stay = $derived(trip.stay ?? allStay);
    const last = $derived(trip.startDate && addDays(trip.startDate, multi ? trip.days + (trip.restAfter?.length ?? 0) - 1 : 0));
    const range = $derived(!trip.startDate || !last ? 'Any date' : last === trip.startDate ? weekdayLabel(last, true)
        : `${weekdayLabel(trip.startDate, trip.startDate.slice(0, 7) !== last.slice(0, 7))} – ${weekdayLabel(last, true)}`);
    // The cell is narrow: "19–25 Jun", or "29 Jun – 3 Jul" across two months.
    const shortRange = $derived.by(() => {
        if (!trip.startDate || !last) return 'Any date';
        if (last === trip.startDate) return weekdayLabel(last, true);
        const day = (date: string) => Number(date.slice(8));
        const month = (date: string) => new Date(date).toLocaleDateString('en-GB', { month: 'short', timeZone: 'UTC' });
        return trip.startDate.slice(0, 7) === last.slice(0, 7) ? `${day(trip.startDate)}–${day(last)} ${month(last)}`
            : `${day(trip.startDate)} ${month(trip.startDate)} – ${day(last)} ${month(last)}`;
    });

    function changeStay(next: Stay[]) {
        onChange({ ...trip, stay: next.length === allStay.length ? undefined : next },
            `Sleeping in ${kinds.filter(k => next.includes(k.kind)).map(k => k.label.toLowerCase()).join(', ')}`);
    }

    function close() {
        dating = false;
        root?.querySelector<HTMLElement>('.date')?.focus();
    }
</script>

<svelte:window onpointerdown={(event) => { if (dating && !root?.contains(event.target as Node)) dating = false; }} />

<div class="trip-row" class:any-stay={!trip.stay} class:multi>
    <DayCount {trip} {line} onChange={onDays} />
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dates" bind:this={root} onkeydown={(event) => { if (event.key === 'Escape' && dating) { event.stopPropagation(); close(); } }}>
        <button type="button" class="cell date" class:unset={!trip.startDate} aria-label={`Trip dates: ${range}`} aria-expanded={dating} onclick={() => dating = !dating}>
            {shortRange}
        </button>
        {#if dating}
            <div class="popover">
                <label>First day
                    <input type="date" value={trip.startDate ?? ''} onchange={(event) => {
                        const date = event.currentTarget.value;
                        onChange({ ...trip, startDate: date || undefined }, date ? 'Dates set' : 'Dates cleared');
                    }} />
                </label>
                <div class="popover-buttons">
                    {#if trip.startDate}<button type="button" class="clear" onclick={() => { onChange({ ...trip, startDate: undefined }, 'Dates cleared'); close(); }}>Clear dates</button>{/if}
                    <button type="button" class="done" onclick={close}>Done</button>
                </div>
            </div>
        {/if}
    </div>
    {#if multi}<StayChip value={stay} onChange={changeStay} />{/if}
</div>

<style>
    /* Fixed columns, so a long value never wraps the bar. The cells have no icons: three do not fit at the default width. */
    .trip-row {
        position: relative;
        display: grid;
        grid-template-columns: minmax(0, 1.15fr) minmax(0, 1fr);
        gap: 4px;
        font-size: 13px;
        color: var(--ink);
    }
    .trip-row.multi { grid-template-columns: minmax(0, 1.15fr) minmax(0, 1fr) minmax(0, 1fr); }
    .trip-row :global(.stay-chip > .panel) { grid-column: 1 / -1; }
    /* The date and the stay chip share the cell form of the day count. */
    .cell, .trip-row :global(.stay-chip > .planner-chip) {
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 6px;
        width: 100%;
        min-width: 0;
        height: 44px;
        padding: 0 6px;
        overflow: hidden;
        border: 0;
        border-radius: 8px;
        background: var(--parchment-2);
        font: 600 13px var(--sans);
        color: var(--ink);
        white-space: nowrap;
    }
    .trip-row :global(.stay-chip > .planner-chip > svg) { display: none; }
    .cell:hover, .trip-row :global(.stay-chip > .planner-chip:hover) { background: color-mix(in srgb, var(--ink) 8%, var(--parchment-2)); }
    .cell[aria-expanded="true"], .trip-row :global(.stay-chip > .planner-chip[aria-expanded="true"]) { outline: 2px solid var(--ink); outline-offset: -2px; }
    .cell.unset, .any-stay :global(.stay-chip > .planner-chip) { color: var(--ink-soft); }
    button {
        font: inherit;
        color: inherit;
        cursor: pointer;
    }
    button:disabled { cursor: default; opacity: .45; }
    button:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .dates { min-width: 0; }
    .popover {
        position: absolute;
        z-index: 40;
        /* Under the start of the bar, not of the date cell: the cell is the second one, and a phone is narrow. */
        top: 48px;
        left: 0;
        width: min(280px, 100%);
        box-sizing: border-box;
        padding: 14px;
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
        color: var(--ink);
    }
    label {
        display: flex;
        flex-direction: column;
        gap: 6px;
        font-size: 14px;
        font-weight: 600;
    }
    input {
        height: 34px;
        padding: 0 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 600 14px var(--sans);
    }
    .popover-buttons {
        display: flex;
        align-items: center;
        gap: 8px;
        margin-top: 12px;
    }
    .clear {
        padding: 0;
        border: 0;
        background: none;
        color: var(--coral);
        font-weight: 600;
    }
    .done {
        margin-left: auto;
        height: 32px;
        padding: 0 14px;
        border: 0;
        border-radius: 6px;
        background: var(--amber);
        color: var(--on-amber);
        font-weight: 600;
    }
</style>
