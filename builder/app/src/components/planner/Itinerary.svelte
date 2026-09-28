<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import DayRow from './DayRow.svelte';
    import type { Day, ItineraryDay, OvernightCandidate, Place, RoutePoint, Trip } from '../../lib/planner/editor';

    let {
        trip, itinerary, days, theme, expandedDay, changing, candidates, conflicts, selectedId,
        onToggle, onInspect, onShowEnd, onSelectPlace, onPick, onChangeOvernight, onEditTarget, onShowConflict,
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
        onToggle: (ridingDay: number) => void;
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

    let restEditing = $state<number | null>(null);
    const scale = $derived(Math.max(trip.limit, ...days.map(day => day.distance)) || 1);
    const calendar = $derived(Object.fromEntries(itinerary.filter(d => !d.rest).map(d => [d.ridingNumber, d.number])));

    function conflictOf(ridingDay: number) {
        return conflicts.find(([, after]) => after.night === ridingDay) ?? null;
    }

    function nameRest(index: number, name: string) {
        restEditing = null;
        onNameRest(index, name);
    }
</script>

<div class="itinerary">
    {#each itinerary as day (`${day.number}-${day.rest}`)}
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
                    <small>{day.pinned?.label ?? 'Open overnight'}</small>
                </div>
                <button type="button" class="icon" aria-label={`Remove rest day ${day.number}`} onclick={() => onRemoveRest(day.restIndex!)}><Icon name="close" size={15} /></button>
            </div>
        {:else}
            {@const conflict = conflictOf(day.ridingNumber)}
            <DayRow
                {trip} {day} {days} {theme} {scale} {changing} {selectedId}
                expanded={expandedDay === day.ridingNumber}
                candidates={expandedDay === day.ridingNumber ? candidates : []}
                conflict={conflict ? calendar[conflict[0].night!] : null}
                onToggle={() => onToggle(day.ridingNumber)}
                {onInspect} {onSelectPlace} {onPick} {onChangeOvernight} {onEditTarget}
                onShowEnd={() => onShowEnd(day)}
                onShowConflict={() => conflict && onShowConflict(conflict)}
            />
            {#if day.ridingNumber < days.length && !trip.restAfter?.includes(day.ridingNumber)}
                <button type="button" class="add-rest" onclick={() => onAddRest(day.ridingNumber)}><Icon name="plus" size={13} />Add rest day</button>
            {/if}
        {/if}
    {/each}
</div>

<style>
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
        margin-left: 44px;
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
