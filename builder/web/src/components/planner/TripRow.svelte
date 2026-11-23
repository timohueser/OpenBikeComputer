<!-- The trip row: the shape of the route, the day count and the dates, as three wells in one line. The day cards show
     the dates, so the row shows none. -->
<script lang="ts">
    import DayCount from './DayCount.svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import { radioKey } from '../../lib/ui/Segmented.svelte';
    import { isThereAndBack, multiDay, oneWay, thereAndBack, type Trip } from '../../lib/planner/editor';
    import { addDays, weekdayLabel } from '../../lib/planner/layers/data-layer';
    import type { RoutingLine } from '../../lib/planner/routing';

    let { trip, line, onChange, onShape, onDays, onLimit }: {
        trip: Trip; line: RoutingLine | undefined;
        onChange: (next: Trip, description: string) => void;
        /** A change of the shape, which adds or removes route points. */
        onShape: (next: Trip, description: string) => void;
        /** A change of the day count, which can change the type of the route. */
        onDays: (next: Trip, description: string) => void;
        /** A press of + at the most days. */
        onLimit: () => void;
    } = $props();

    let dating = $state(false);
    let root = $state<HTMLDivElement>();
    const back = $derived(isThereAndBack(trip));
    const loop = $derived(!!trip.loop && !back);
    // The first shape is the route as planned, and its word names what the route is: a loop is no third choice.
    const shapes = $derived([
        { icon: loop ? 'loop' : 'one-way', label: loop ? 'Loop' : 'One way', chosen: !back,
            choose: () => onShape(oneWay(trip), 'One way · the route ends at the far point') },
        { icon: 'back', label: 'There and back', chosen: back,
            choose: () => onShape(thereAndBack(trip), 'There and back · the route returns the same way') },
    ]);
    const last = $derived(trip.startDate && addDays(trip.startDate, multiDay(trip) ? trip.days - 1 : 0));
    const range = $derived(!trip.startDate || !last ? 'Any date' : last === trip.startDate ? weekdayLabel(last, true)
        : `${weekdayLabel(trip.startDate, trip.startDate.slice(0, 7) !== last.slice(0, 7))} – ${weekdayLabel(last, true)}`);

    function choose(index: number) {
        if (!shapes[index].chosen) shapes[index].choose();
    }

    function close() {
        dating = false;
        root?.querySelector<HTMLElement>('.date')?.focus();
    }
</script>

<svelte:window onpointerdown={(event) => { if (dating && !root?.contains(event.target as Node)) dating = false; }} />

<div class="trip-row">
    <div class="well shape" role="radiogroup" aria-label="Route shape">
        {#each shapes as { icon, label, chosen }, index (index)}
            <button type="button" role="radio" aria-checked={chosen} aria-label={chosen ? undefined : label} title={chosen ? undefined : label}
                tabindex={chosen ? 0 : -1} onclick={() => choose(index)} onkeydown={(event) => radioKey(event, index, shapes.length, choose)}>
                <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    {#if icon === 'loop'}
                        <path d="M5 19c-3-6 1-14 8-14s9 8 4 12-9 2-12 2Z" /><circle cx="5" cy="19" r="1.7" fill="currentColor" />
                    {:else}
                        <path d="M5 19c3-3 3-8 7-9s5-3 7-5" /><circle cx="5" cy="19" r="1.7" fill="currentColor" />
                        {#if icon === 'back'}<circle cx="19" cy="5" r="1.7" />{:else}<path d="M14.5 4.5H19.5V9.5" />{/if}
                    {/if}
                </svg>{#if chosen}{label}{/if}
            </button>
        {/each}
    </div>
    <DayCount {trip} {line} onChange={onDays} {onLimit} />
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dates" bind:this={root} onkeydown={(event) => { if (event.key === 'Escape' && dating) { event.stopPropagation(); close(); } }}>
        <button type="button" class="well date" class:set={!!trip.startDate} aria-label={`Trip dates: ${range}`} title={`Trip dates: ${range}`} aria-expanded={dating} onclick={() => dating = !dating}>
            <Icon name="calendar" size={16} />
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
</div>

<style>
    /* The wells wrap in a narrow side panel. */
    .trip-row {
        position: relative;
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
    }
    /* The look of the three wells; the day count (`DayCount`) has the same one. */
    .well {
        display: inline-flex;
        align-items: center;
        height: 36px;
        border: 0;
        border-radius: 8px;
        background: var(--parchment-2);
        color: var(--ink);
        font: 600 13px var(--sans);
    }
    button { cursor: pointer; }
    button:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .shape { padding: 2px; gap: 2px; }
    .shape button {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        height: 32px;
        padding: 0 9px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        font: inherit;
        white-space: nowrap;
    }
    .shape button:hover { color: var(--ink); }
    .shape button[aria-checked="true"] {
        background: var(--panel);
        color: var(--ink);
        box-shadow: inset 0 0 0 1px var(--line-strong);
    }
    .shape button:focus-visible { outline-offset: 0; }
    .date {
        width: 36px;
        padding: 0;
        justify-content: center;
        color: var(--ink-soft);
    }
    .date.set { color: var(--ink); }
    .date:hover { background: color-mix(in srgb, var(--ink) 8%, var(--parchment-2)); }
    .date[aria-expanded="true"] { outline: 2px solid var(--ink); outline-offset: -2px; }
    .popover {
        position: absolute;
        z-index: 40;
        /* Under the start of the row, not of the date well: the well is the last one, and a phone is narrow. */
        top: 100%;
        left: 0;
        width: min(280px, 100%);
        box-sizing: border-box;
        margin-top: 4px;
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
    .clear, .done { font: 600 13px var(--sans); }
    .clear {
        padding: 0;
        border: 0;
        background: none;
        color: var(--coral);
    }
    .done {
        margin-left: auto;
        height: 32px;
        padding: 0 14px;
        border: 0;
        border-radius: 6px;
        background: var(--amber);
        color: var(--on-amber);
    }
</style>
