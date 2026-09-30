<script lang="ts">
    import { tick } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import DayRow from './DayRow.svelte';
    import type { Day, ItineraryDay, OvernightCandidate, Place, RoutePoint, Trip } from '../../lib/planner/editor';

    let {
        trip, itinerary, days, theme, expandedDay, changing, candidates, conflicts, selectedId, revealId, hoveredId = null, onHover,
        onToggle, onOverview, onInspect, onShowEnd, onSelectPlace, onPick, onChangeOvernight, onEditTarget, onShowConflict,
        onAddRest, onRemoveRest, onNameRest,
    }: {
        trip: Trip;
        itinerary: ItineraryDay[];
        days: Day[];
        theme: 'light' | 'dark';
        /** Riding number of the open day. */
        expandedDay: number | null;
        changing: boolean;
        candidates: OvernightCandidate[];
        conflicts: [RoutePoint, RoutePoint][];
        selectedId: string | null;
        hoveredId?: string | null;
        onHover?: (id: string | null) => void;
        /** The point whose row lights up for a moment. */
        revealId: string | null;
        onToggle: (ridingDay: number) => void;
        onOverview: () => void;
        onInspect: (point: RoutePoint) => void;
        onShowEnd: (day: Day) => void;
        onSelectPlace: (place: Place) => void;
        onPick: () => void;
        onChangeOvernight: (changing: boolean) => void;
        onEditTarget: () => void;
        onShowConflict: (points: [RoutePoint, RoutePoint]) => void;
        onAddRest: (afterRidingDay: number) => void;
        onRemoveRest: (index: number) => void;
        onNameRest: (index: number, name: string) => void;
    } = $props();

    let root: HTMLDivElement;
    let dayPicker = $state<HTMLSelectElement>();
    let restEditing = $state<number | null>(null);
    const focused = $derived(itinerary.find(day => !day.rest && day.ridingNumber === expandedDay));
    const visible = $derived(focused ? itinerary.filter(day => day.ridingNumber === focused.ridingNumber) : itinerary);
    const ridingDays = $derived(itinerary.filter(day => !day.rest));
    const scale = $derived(Math.max(trip.limit, ...days.map(day => day.distance)) || 1);
    const calendar = $derived(Object.fromEntries(itinerary.filter(d => !d.rest).map(d => [d.ridingNumber, d.number])));

    $effect(() => {
        expandedDay;
        tick().then(() => {
            const scroll = root?.closest('.pane-scroll');
            if (scroll) scroll.scrollTop = 0;
        });
    });

    async function openDay(riding: number) {
        onToggle(riding);
        await tick();
        dayPicker?.focus({ preventScroll: true });
    }

    async function overview() {
        const riding = expandedDay;
        onOverview();
        await tick();
        root?.querySelector<HTMLElement>(`[data-day="${riding}"]`)?.focus();
    }

    function conflictOf(ridingDay: number) {
        return conflicts.find(([, after]) => after.night === ridingDay) ?? null;
    }

    function nameRest(index: number, name: string) {
        restEditing = null;
        onNameRest(index, name);
    }
</script>

{#if focused}
    <nav class="day-nav" aria-label="Day navigation">
        <button type="button" class="overview" onclick={overview}><Icon name="back" size={16} />All days</button>
        <div class="day-picker">
            <button type="button" class="icon" aria-label="Previous riding day" disabled={focused.ridingNumber === 1} onclick={() => onToggle(focused.ridingNumber - 1)}><Icon name="back" size={16} /></button>
            <select bind:this={dayPicker} aria-label="Selected day" value={focused.ridingNumber} onchange={event => onToggle(Number(event.currentTarget.value))}>
                {#each ridingDays as day}<option value={day.ridingNumber}>Day {day.number}</option>{/each}
            </select>
            <button type="button" class="icon" aria-label="Next riding day" disabled={focused.ridingNumber === days.length} onclick={() => onToggle(focused.ridingNumber + 1)}><Icon name="arrow" size={16} /></button>
        </div>
    </nav>
{/if}
<div class="itinerary" bind:this={root}>
    {#if !focused}<p class="hint">Open a day to plan its stops and overnight.</p>{/if}
    {#key expandedDay}
    {#each visible as day (`${day.number}-${day.rest}`)}
        {#if day.rest}
            <div class="rest">
                <span class="badge"><Icon name="pause" size={14} /></span>
                <div class="rest-text">
                    {#if restEditing === day.restIndex}
                        <!-- svelte-ignore a11y_autofocus -->
                        <input aria-label={`Name rest day ${day.number}`} value={trip.restNames?.[day.restIndex!] ?? ''} placeholder="Rest day name" autofocus
                            onblur={(event) => nameRest(day.restIndex!, event.currentTarget.value)}
                            onkeydown={(event) => {
                                if (event.key === 'Enter') event.currentTarget.blur();
                                if (event.key === 'Escape') restEditing = null;
                            }} />
                    {:else}
                        <button type="button" class="rest-name" aria-label={`Name rest day ${day.number}`} onclick={() => restEditing = day.restIndex!}>
                            <strong>Day {day.number} · {trip.restNames?.[day.restIndex!] || 'Rest'}</strong><Icon name="pencil" size={13} />
                        </button>
                    {/if}
                    <small>{day.pinned?.label ?? `Night ${calendar[day.ridingNumber]} not chosen`}</small>
                </div>
                <button type="button" class="icon" aria-label={`Remove rest day ${day.number}`} onclick={() => onRemoveRest(day.restIndex!)}><Icon name="close" size={15} /></button>
            </div>
        {:else}
            {@const conflict = conflictOf(day.ridingNumber)}
            <DayRow
                {trip} {day} {days} {theme} {scale} {changing} {selectedId} {revealId} {hoveredId} {onHover} {calendar}
                expanded={expandedDay === day.ridingNumber}
                candidates={expandedDay === day.ridingNumber ? candidates : []}
                conflict={conflict ? calendar[conflict[0].night!] : null}
                onToggle={() => openDay(day.ridingNumber)}
                {onInspect} {onSelectPlace} {onPick} {onChangeOvernight} {onEditTarget}
                onShowEnd={() => onShowEnd(days[day.ridingNumber - 1])}
                onShowConflict={() => conflict && onShowConflict(conflict)}
            />
            {#if focused && day.ridingNumber < days.length && !trip.restAfter?.includes(day.ridingNumber)}
                <button type="button" class="add-rest" onclick={() => onAddRest(day.ridingNumber)}><Icon name="plus" size={13} />Add rest day</button>
            {/if}
        {/if}
    {/each}
    {/key}
</div>

<style>
    .day-nav {
        position: sticky;
        top: 0;
        z-index: 2;
        display: flex;
        justify-content: space-between;
        align-items: center;
        gap: 8px;
        padding: 8px 16px;
        border-bottom: 1px solid var(--line);
        background: var(--panel);
    }
    .overview, .day-picker { display: flex; align-items: center; gap: 8px; }
    .overview { min-height: 36px; padding: 6px 10px; border: 1px solid var(--line-strong); border-radius: 6px; background: var(--panel); color: var(--ink); font-size: 13px; font-weight: 600; }
    .overview:hover { background: var(--parchment-2); }
    .day-picker { gap: 2px; }
    select { height: 34px; padding: 0 4px; border: 0; border-radius: 6px; color: var(--ink); background: var(--panel); font: 600 14px var(--sans); }
    .hint { margin: 4px 8px 8px; font-size: 13px; color: var(--ink-soft); }

    .itinerary {
        display: flex;
        flex-direction: column;
        gap: 4px;
        padding: 4px 8px 16px;
    }
    button {
        border: 0;
        background: none;
        color: inherit;
        font: inherit;
        cursor: pointer;
    }
    .rest {
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 8px;
        color: var(--ink);
    }
    .badge {
        width: 24px;
        height: 24px;
        flex: none;
        display: grid;
        place-items: center;
        border: 1.5px dashed var(--line-strong);
        border-radius: 50%;
        color: var(--ink-soft);
    }
    .rest-text {
        flex: 1;
        min-width: 0;
    }
    .rest-name {
        display: flex;
        align-items: center;
        gap: 8px;
        text-align: left;
    }
    .rest-name strong {
        font: 600 14px var(--sans);
    }
    .rest-name :global(svg) {
        color: var(--ink-faint);
    }
    .rest small {
        display: block;
        margin-top: 2px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .rest input {
        width: 100%;
        height: 30px;
        padding: 0 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font-size: 14px;
    }
    .icon {
        display: grid;
        place-items: center;
        width: 30px;
        height: 30px;
        border-radius: 6px;
        color: var(--ink-soft);
    }
    .icon:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .add-rest {
        display: flex;
        align-items: center;
        gap: 8px;
        align-self: flex-start;
        margin-left: 8px;
        padding: 4px 8px;
        border-radius: 6px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .add-rest:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
</style>
