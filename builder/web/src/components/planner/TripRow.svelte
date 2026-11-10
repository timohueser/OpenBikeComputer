<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import StayChip from './StayChip.svelte';
    import { allStay, fewestDays, maxRidingDays, setDays, type Stay, type Trip } from '../../lib/planner/editor';
    import { addDays, weekdayLabel } from '../../lib/planner/layers/data-layer';
    import { stayOptions as kinds } from '../../lib/planner/search/presentation';

    let { trip, onChange }: { trip: Trip; onChange: (next: Trip, description: string) => void } = $props();

    let dating = $state(false);
    let root = $state<HTMLDivElement>();
    const stay = $derived(trip.stay ?? allStay);
    const range = $derived.by(() => {
        if (!trip.startDate) return 'Start date';
        const last = addDays(trip.startDate, trip.days + (trip.restAfter?.length ?? 0) - 1);
        return `${weekdayLabel(trip.startDate, trip.startDate.slice(0, 7) !== last.slice(0, 7))} – ${weekdayLabel(last, true)}`;
    });

    function changeDays(days: number) {
        onChange(setDays(trip, days), `${days} riding ${days === 1 ? 'day' : 'days'}`);
    }

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

<div class="trip-row">
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dates" bind:this={root} onkeydown={(event) => { if (event.key === 'Escape' && dating) { event.stopPropagation(); close(); } }}>
        <button type="button" class="planner-chip date" class:guess={!trip.startDate} aria-label={`Trip dates: ${range}`} aria-expanded={dating} onclick={() => dating = !dating}>
            <Icon name="calendar" size={14} />{range}
        </button>
        {#if dating}
            <div class="popover">
                <label>First riding day
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
    <div class="days" role="group" aria-label="Number of riding days">
        <button type="button" aria-label="One day fewer" disabled={trip.days <= fewestDays(trip)} onclick={() => changeDays(trip.days - 1)}><Icon path="M5 12h14" size={15} /></button>
        <span class="count"><span class:guess={!trip.daysSet}>{trip.days} {trip.days === 1 ? 'day' : 'days'}</span></span>
        <button type="button" aria-label="One day more" disabled={trip.days >= maxRidingDays} onclick={() => changeDays(trip.days + 1)}><Icon name="plus" size={15} /></button>
    </div>
    <StayChip value={stay} onChange={changeStay} />
</div>

<style>
    .trip-row {
        display: flex;
        align-items: center;
        gap: 8px;
        flex-wrap: wrap;
        font-size: 13px;
        color: var(--ink-soft);
    }
    button {
        font: inherit;
        color: inherit;
        cursor: pointer;
    }
    button:disabled { cursor: default; opacity: .45; }
    button:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .dates { position: relative; }
    .date {
        --chip-ink: var(--query-time);
        white-space: nowrap;
    }
    /* Outranks the global `.planner-shell .planner-chip`, whose stylesheet loads later. */
    .date.planner-chip.guess {
        border: 1.5px dashed var(--ink-soft);
        color: var(--ink-soft);
    }
    .popover {
        position: absolute;
        z-index: 40;
        top: 40px;
        left: 0;
        width: 280px;
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
    .days {
        display: flex;
        align-items: center;
        gap: 2px;
        box-sizing: border-box;
        height: 32px;
        padding: 2px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
    }
    .days button {
        display: grid;
        place-items: center;
        width: 26px;
        height: 26px;
        padding: 0;
        border: 0;
        border-radius: 4px;
        background: none;
        color: var(--ink);
    }
    .days button:not(:disabled):hover { background: var(--parchment-2); }
    .count {
        min-width: 56px;
        text-align: center;
        font-size: 14px;
        color: var(--ink);
        font-variant-numeric: tabular-nums;
    }
    .count span {
        padding-bottom: 1px;
        border-bottom: 2px solid var(--ink);
        font-weight: 700;
    }
    .count span.guess {
        border-bottom: 1.5px dashed var(--ink-soft);
        font-weight: 600;
    }
</style>
