<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import PlaceRow from './PlaceRow.svelte';
    import { dayColor } from '../../lib/planner/day-colors';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import { profileAscent } from '../../lib/planner/profile-data';
    import { dayOverTarget, dayStops, places, type ItineraryDay, type OvernightCandidate, type Place, type RoutePoint, type Trip, type Day } from '../../lib/planner/editor';

    let {
        trip, day, days, theme, scale, expanded, changing, candidates, conflict, selectedId,
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
    const start = $derived(riding === 1 ? trip.points.find(p => p.kind === 'start')!.label : previous?.pinned?.label ?? 'Open overnight');
    const end = $derived(last ? trip.points.find(p => p.kind === 'finish')!.label : day.pinned?.label ?? 'Choose overnight');
    const endPlace = $derived(day.pinned && places.find(p => p.coordinate[0] === day.pinned!.coordinate[0] && p.coordinate[1] === day.pinned!.coordinate[1]));
    const ascent = $derived(profileAscent(day.from, day.to));
    const over = $derived(dayOverTarget(trip, day, ascent));
    const confirmed = $derived((riding === 1 || !!previous?.pinned) && (last || !!day.pinned));
    const overParts = $derived([
        ...(over.km > 0 ? [`${over.km.toFixed(1)} km over target`] : []),
        ...(over.climb > 0 ? [`↑ ${over.climb} m over target`] : []),
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
        return `${Math.floor(hours)}h ${Math.round(hours % 1 * 60)}m`;
    }

    function candidateDay(candidate: OvernightCandidate) {
        const excess = dayOverTarget(trip, candidate, candidate.ascent);
        return { distance: candidate.distance, ascent: candidate.ascent, over: excess.km > 0 || excess.climb > 0 };
    }
</script>

<section class="day" class:expanded style:--day-color={dayColor(riding, theme)}>
    <button type="button" class="heading" onclick={onToggle} aria-expanded={expanded}>
        <span class="badge">{day.number}</span>
        <span class="title">
            <strong>{start} → {end}</strong>
            <small>{duration(day.hours)} · ↑ {ascent} m{endPlace ? ` · ${placeCategories[endPlace.category].label}` : ''}</small>
        </span>
        <span class="distance" class:over={over.km > 0}>{day.distance.toFixed(1)}<small>km</small></span>
        <Icon name={expanded ? 'down' : 'chevron'} size={14} />
    </button>
    <div class="bar" aria-hidden="true">
        <span class="fill" style:width={`${Math.min(100, day.distance / scale * 100)}%`}></span>
        {#if trip.limit > 0}<span class="tick" style:left={`${trip.limit / scale * 100}%`}></span>{/if}
    </div>
    {#if confirmed && overParts.length}
        <p class="note">{overParts.join(' · ')} · <button type="button" class="link" onclick={onEditTarget}>Edit target</button></p>
    {/if}
    {#if conflict !== null}
        <p class="note warn"><Icon name="warning" size={15} />Ends before day {conflict}'s overnight · <button type="button" class="link" onclick={onShowConflict}>Show</button></p>
    {/if}
    {#if expanded}
        <div class="body">
            <div class="stop"><span class="dot"></span><span>{start}</span><small>Start</small></div>
            {#each stops as { point, km } (point.id)}
                <button type="button" class="stop" class:chosen={selectedId === point.id} onclick={() => onInspect(point)}>
                    <Icon name={stopKinds[point.kind].icon} size={15} /><span>{point.label}</span>
                    <small>{stopKinds[point.kind].label} · at {km.toFixed(1)} km</small>
                </button>
            {/each}
            {#if last || day.pinned}
                <button type="button" class="stop end" onclick={onShowEnd}>
                    <Icon name={last ? 'flag' : 'camp'} size={15} /><span>{end}</span><small>{last ? 'Finish' : 'Pinned'}</small>
                </button>
            {/if}
            {#if choosing}
                <div class="choose">
                    <div class="choose-head">
                        <button type="button" onclick={onShowEnd}><strong>Choose overnight</strong></button>
                        <small>Day {day.number} would be</small>
                    </div>
                    {#each candidates as candidate (candidate.place.id)}
                        <PlaceRow place={candidate.place} day={candidateDay(candidate)} selected={selectedId === candidate.place.id} onSelect={onSelectPlace} />
                    {/each}
                    <div class="choose-actions">
                        <button type="button" class="link" onclick={onPick}>Pick another spot on the map</button>
                        {#if day.pinned}<button type="button" class="link quiet" onclick={() => onChangeOvernight(false)}>Cancel</button>{/if}
                    </div>
                </div>
            {:else if day.pinned && !last}
                <button type="button" class="link change" onclick={() => onChangeOvernight(true)}>Change overnight</button>
            {/if}
        </div>
    {/if}
</section>

<style>
    .day {
        padding: 4px 8px 12px;
        border-radius: 8px;
    }
    .expanded {
        background: var(--parchment-2);
    }
    button {
        border: 0;
        background: none;
        color: inherit;
        font: inherit;
        cursor: pointer;
    }
    .heading {
        display: flex;
        align-items: center;
        gap: 12px;
        width: 100%;
        padding: 8px 0 8px;
        text-align: left;
        color: var(--ink);
    }
    .heading > :global(svg) {
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
        position: relative;
        height: 4px;
        margin: 0 26px 0 36px;
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
        align-items: center;
        gap: 4px;
        margin: 8px 0 0 36px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .warn {
        color: var(--coral);
    }
    .warn :global(svg) {
        margin-right: 2px;
    }
    .link {
        padding: 0;
        color: var(--forest);
        font-weight: 600;
        text-decoration: underline;
        text-underline-offset: 3px;
    }
    .link.quiet {
        color: var(--ink-soft);
    }
    .body {
        margin: 12px 0 0 36px;
    }
    .stop {
        display: flex;
        align-items: center;
        gap: 10px;
        width: 100%;
        min-height: 32px;
        text-align: left;
        font-size: 14px;
        color: var(--ink);
    }
    .stop > span:not(.dot) {
        flex: 1;
        min-width: 0;
    }
    .stop small {
        font-size: 13px;
        color: var(--ink-soft);
    }
    .stop > :global(svg) {
        flex: none;
        color: var(--ink-soft);
    }
    button.stop:hover > span {
        text-decoration: underline;
        text-underline-offset: 3px;
    }
    .stop.chosen {
        font-weight: 600;
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
        margin-top: 12px;
    }
    .choose-head {
        display: flex;
        align-items: baseline;
        justify-content: space-between;
        padding-right: 22px;
        margin-bottom: 4px;
    }
    .choose-head strong {
        font: 600 14px var(--sans);
    }
    .choose-head small {
        font-size: 11px;
        color: var(--ink-soft);
    }
    .choose-actions {
        display: flex;
        justify-content: space-between;
        margin-top: 8px;
        font-size: 13px;
    }
    .change {
        margin-top: 8px;
        font-size: 13px;
    }
</style>
