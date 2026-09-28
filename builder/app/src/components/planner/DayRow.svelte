<script lang="ts">
    import type { Attachment } from 'svelte/attachments';
    import Icon from './PlannerIcon.svelte';
    import PlaceRow from './PlaceRow.svelte';
    import RouteStats from './RouteStats.svelte';
    import { dayColor } from '../../lib/planner/day-colors';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import { profileAscent } from '../../lib/planner/profile-data';
    import { dayOverTarget, dayStops, places, type ItineraryDay, type OvernightCandidate, type Place, type RoutePoint, type Trip, type Day } from '../../lib/planner/editor';

    let {
        trip, day, days, theme, scale, expanded, changing, candidates, conflict, selectedId, revealId, calendar,
        onToggle, onInspect, onShowEnd, onSelectPlace, onPick, onChangeOvernight, onEditTarget, onShowConflict,
    }: {
        trip: Trip;
        day: ItineraryDay;
        days: Day[];
        theme: 'light' | 'dark';
        /** Kilometres the full length bar stands for. */
        scale: number;
        expanded: boolean;
        changing: boolean;
        candidates: OvernightCandidate[];
        /** The calendar day whose overnight this day ends before. */
        conflict: number | null;
        selectedId: string | null;
        /** The point whose row scrolls into view and lights up for a moment. */
        revealId: string | null;
        /** Riding number → calendar number. */
        calendar: Record<number, number>;
        onToggle: () => void;
        onInspect: (point: RoutePoint) => void;
        onShowEnd: () => void;
        onSelectPlace: (place: Place) => void;
        onPick: () => void;
        onChangeOvernight: (changing: boolean) => void;
        onEditTarget: () => void;
        onShowConflict: () => void;
    } = $props();

    const riding = $derived(day.ridingNumber);
    const last = $derived(riding === days.length);
    const previous = $derived(days[riding - 2]);
    const start = $derived(riding === 1 ? trip.points.find(p => p.kind === 'start')!.label : previous?.pinned?.label ?? `Day ${calendar[riding - 1]} overnight`);
    const end = $derived(last ? trip.points.find(p => p.kind === 'finish')!.label : day.pinned?.label ?? 'Overnight to choose');
    const endPlace = $derived(day.pinned && places.find(p => p.coordinate[0] === day.pinned!.coordinate[0] && p.coordinate[1] === day.pinned!.coordinate[1]));
    const ascent = $derived(profileAscent(day.from, day.to));
    const over = $derived(dayOverTarget(trip, day, ascent));
    // Both ends chosen: the figures are what the rider will ride. Otherwise they are a suggestion, shown with ≈.
    const confirmed = $derived((riding === 1 || !!previous?.pinned) && (last || !!day.pinned));
    const overParts = $derived([
        ...(over.km > 0 ? [confirmed ? `${over.km.toFixed(1)} km over target` : `≈ ${Math.max(1, Math.round(over.km))} km over target`] : []),
        ...(over.climb > 0 ? [`${confirmed ? '' : '≈ '}↑ ${over.climb} m over target`] : []),
    ]);
    const choosing = $derived(!last && (!day.pinned || changing));
    const stops = $derived(expanded ? dayStops(trip, day) : []);
    const stopKinds: Record<string, { label: string; icon: string }> = {
        waypoint: { label: 'Visit', icon: 'flag' },
        detour: { label: 'Out and back', icon: 'back' },
        pass: { label: 'Pass', icon: 'pin' },
        marker: { label: 'Marker', icon: 'pin' },
    };

    function duration(hours: number) {
        const minutes = Math.round(hours * 60);
        return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
    }

    function candidateDay(candidate: OvernightCandidate) {
        const excess = dayOverTarget(trip, candidate, candidate.ascent);
        return { distance: candidate.distance, ascent: candidate.ascent, over: excess.km > 0 || excess.climb > 0 };
    }

    function reveal(id: string): Attachment {
        return (node) => {
            if (revealId === id) node.scrollIntoView({ block: 'nearest', behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' });
        };
    }
</script>

<section class="day" class:expanded style:--day-color={dayColor(riding, theme)}>
    {#if expanded}
        <header class="detail-heading">
            <span class="badge">{day.number}</span>
            <h2>{start} → {end}</h2>
        </header>
        <RouteStats distance={day.distance} {ascent} hours={day.hours} />
    {:else}
    <button type="button" class="heading" data-day={riding} onclick={onToggle} aria-label={`Open day ${day.number}: ${start} to ${end}`}>
        <span class="heading-content">
        <span class="badge">{day.number}</span>
        <span class="title">
            <strong>{start} → {end}</strong>
            <small>{duration(day.hours)} · <span class:over={over.climb > 0}>↑ {ascent} m</span>{endPlace ? ` · ${placeCategories[endPlace.category].label}` : ''}</small>
        </span>
        <span class="distance" class:over={over.km > 0}>{day.distance.toFixed(1)}<small>km</small></span>
        <Icon name="chevron" size={14} />
        </span>
    <span class="bar" aria-hidden="true">
        <span class="fill" style:width={`${Math.min(100, day.distance / scale * 100)}%`}></span>
        {#if trip.limit > 0}<span class="tick" style:left={`${trip.limit / scale * 100}%`}></span>{/if}
    </span>
    </button>
    {/if}
    {#if overParts.length}
        <p class="note">{overParts.join(' · ')} · <button type="button" class="planner-action" onclick={onEditTarget}>Edit target</button></p>
    {/if}
    {#if conflict !== null}
        <p class="note warn"><Icon name="warning" size={15} />Ends before day {conflict}'s overnight · <button type="button" class="planner-action" onclick={onShowConflict}>Show</button></p>
    {/if}
    {#if expanded}
        <div class="body">
            <h3>Stops along the day</h3>
            {#if stops.length}<div class="stop"><span class="dot"></span><span>{start}</span><small>0 km</small></div>{/if}
            {#each stops as { point, km } (point.id)}
                <button type="button" class="stop" class:chosen={selectedId === point.id} class:flash={revealId === point.id} onclick={() => onInspect(point)} {@attach reveal(point.id)}>
                    <Icon name={stopKinds[point.kind].icon} size={15} /><span>{point.label}</span>
                    <small>{stopKinds[point.kind].label} · at {km.toFixed(1)} km</small>
                </button>
            {/each}
            {#if last || day.pinned}
                <button type="button" class="stop end" class:flash={!!day.pinned && revealId === day.pinned.id} onclick={onShowEnd} {@attach reveal(day.pinned?.id ?? 'finish')}>
                    <Icon name={last ? 'flag' : 'camp'} size={15} /><span>{end}</span><small>{last ? 'Finish' : 'Pinned'}</small>
                </button>
            {/if}
            {#if stops.length && !last && !day.pinned}
                <button type="button" class="stop end" onclick={onShowEnd}><Icon name="camp" size={15} /><span>Suggested day end<small>Choose an overnight below</small></span><small>{day.distance.toFixed(1)} km</small></button>
            {/if}
            <p class="help">{stops.length ? 'Click the map to add a stop. Drag the route to reshape it.' : 'No stops yet. Click the map to add one.'}</p>
            {#if choosing}
                <div class="choose">
                    <div class="choose-head">
                        <h3>Where to sleep</h3>
                        <button type="button" class="planner-action" onclick={onShowEnd}>Show area</button>
                    </div>
                    <p class="help">Distance and climb if you stay here.</p>
                    {#each candidates as candidate (candidate.place.id)}
                        <PlaceRow place={candidate.place} day={candidateDay(candidate)} selected={selectedId === candidate.place.id} onSelect={onSelectPlace} />
                    {:else}
                        <p class="help">No suggested places here. Pick a spot on the map.</p>
                    {/each}
                    <div class="choose-actions">
                        <button type="button" class="planner-action" onclick={onPick}>Pick another spot on the map</button>
                        {#if day.pinned}<button type="button" class="planner-action quiet" onclick={() => onChangeOvernight(false)}>Cancel</button>{/if}
                    </div>
                </div>
            {:else if day.pinned && !last}
                <button type="button" class="planner-action change" onclick={() => onChangeOvernight(true)}>Change overnight</button>
            {/if}
        </div>
    {/if}
</section>

<style>
    .day {
        padding: 4px 8px 12px;
        border-radius: 8px;
    }
    .expanded { padding: 4px 8px 12px; }
    .day:not(.expanded) { border-bottom: 1px solid var(--line); border-radius: 0; }
    .detail-heading { display: flex; align-items: center; gap: 10px; }
    h2 { margin: 0; font: 600 17px/1.35 var(--sans); overflow-wrap: anywhere; }
    h3 { margin: 0; font: 600 14px var(--sans); }
    .help { margin: 6px 0 12px; font-size: 13px; color: var(--ink-soft); }
    .expanded .note { margin-left: 0; }

    button {
        border: 0;
        background: none;
        color: inherit;
        font: inherit;
        cursor: pointer;
    }
    .heading {
        display: block;
        width: calc(100% + 16px);
        margin: 0 -8px;
        padding: 8px;
        border-radius: 6px;
        text-align: left;
        color: var(--ink);
    }
    .heading:hover {
        background: var(--parchment-2);
    }
    .heading-content { display: flex; align-items: center; gap: 12px; }
    .heading-content > :global(svg) {
        flex: none;
        color: var(--ink-faint);
    }
    .badge {
        width: 24px;
        height: 24px;
        flex: none;
        display: grid;
        place-items: center;
        border-radius: 50%;
        background: var(--day-color);
        color: var(--panel);
        font: 700 13px var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .title {
        flex: 1;
        min-width: 0;
    }
    .title strong,
    .title small {
        display: block;
        overflow: hidden;
    }
    .title small {
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .title strong {
        font: 600 14px/1.3 var(--sans);
        display: -webkit-box;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 2;
        line-clamp: 2;
        overflow-wrap: anywhere;
    }
    .title small {
        margin-top: 2px;
        font-size: 13px;
        color: var(--ink-soft);
        font-variant-numeric: tabular-nums;
    }
    .distance {
        flex: none;
        font: 700 17px var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .distance small {
        margin-left: 2px;
        font: 400 13px var(--sans);
        color: var(--ink-soft);
    }
    .over {
        color: var(--coral);
    }
    .bar {
        display: block;
        position: relative;
        height: 4px;
        margin: 8px 26px 4px 36px;
        border-radius: 2px;
        background: var(--parchment-3);
    }
    .fill {
        position: absolute;
        inset: 0 auto 0 0;
        border-radius: 2px;
        background: var(--day-color);
    }
    .tick {
        position: absolute;
        top: -3px;
        width: 2px;
        height: 10px;
        margin-left: -1px;
        border-radius: 1px;
        background: var(--ink-faint);
    }
    .note {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 4px;
        margin: 4px 0 0 36px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .warn {
        color: var(--coral);
    }
    .warn :global(svg) {
        margin-right: 2px;
    }
    .body {
        margin: 4px 0 0;
        padding-top: 12px;
        border-top: 1px solid var(--line);
    }
    .stop {
        display: flex;
        align-items: center;
        gap: 10px;
        width: calc(100% + 16px);
        min-height: 40px;
        margin: 0 -8px;
        padding: 6px 8px;
        border-radius: 6px;
        text-align: left;
        font-size: 14px;
        color: var(--ink);
    }
    button.stop:hover {
        background: var(--parchment-2);
    }
    .stop > span:not(.dot) {
        flex: 1;
        min-width: 0;
    }
    .stop small {
        display: block;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .stop > :global(svg) {
        flex: none;
        color: var(--ink-soft);
    }
    .stop.chosen {
        font-weight: 600;
    }
    .flash {
        animation: flash 1.2s ease-out;
    }
    @keyframes flash {
        from { background: var(--parchment-2); }
        to { background: transparent; }
    }
    @media (prefers-reduced-motion: reduce) {
        .flash {
            animation: none;
            background: var(--parchment-2);
        }
    }
    .dot {
        width: 8px;
        height: 8px;
        margin: 0 4px 0 3px;
        border: 1.5px solid var(--ink-soft);
        border-radius: 50%;
    }
    .end {
        font-weight: 600;
    }
    .choose {
        margin-top: 16px;
        padding-top: 12px;
        border-top: 1px solid var(--line);
    }
    .choose-head {
        display: flex;
        align-items: baseline;
        justify-content: space-between;
        gap: 8px;
    }
    .choose-actions {
        display: flex;
        justify-content: space-between;
        margin-top: 4px;
        font-size: 13px;
    }
    .change {
        margin-top: 4px;
        font-size: 13px;
    }
</style>
