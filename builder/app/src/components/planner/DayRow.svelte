<script lang="ts">
    import type { Attachment } from 'svelte/attachments';
    import Icon from './PlannerIcon.svelte';
    import PlaceRow from './PlaceRow.svelte';
    import { dayColor } from '../../lib/planner/day-colors';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import { profileAscent } from '../../lib/planner/profile-data';
    import { dayOverTarget, dayStops, type ItineraryDay, type OvernightCandidate, type Place, type RoutePoint, type Trip, type Day } from '../../lib/planner/editor';

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
    const start = $derived(riding === 1 ? trip.points.find(p => p.kind === 'start')!.label : previous?.pinned?.label ?? `Night ${calendar[riding - 1]} not chosen`);
    const end = $derived(last ? trip.points.find(p => p.kind === 'finish')!.label : day.pinned?.label ?? `Night ${day.number} not chosen`);

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
        return `${Math.floor(hours)}h ${Math.round(hours % 1 * 60)}m`;
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
    <button type="button" class="heading" onclick={onToggle} aria-expanded={expanded}>
        <span class="badge">{day.number}</span>
        <span class="title">
            <strong>{start} → {end}</strong>
            <small>{duration(day.hours)} · <span class:over={over.climb > 0}>↑ {ascent} m</span>{day.pinned?.placeKind ? ` · ${kindLabel(day.pinned.placeKind)}` : ''}</small>
        </span>
        <span class="distance" class:over={over.km > 0}>{day.distance.toFixed(1)}<small>km</small></span>
        <Icon name={expanded ? 'down' : 'chevron'} size={14} />
    </button>
    <div class="bar" aria-hidden="true">
        <span class="fill" style:width={`${Math.min(100, day.distance / scale * 100)}%`}></span>
        {#if trip.limit > 0}<span class="tick" style:left={`${trip.limit / scale * 100}%`}></span>{/if}
    </div>
    {#if overParts.length}
        <p class="note">{overParts.join(' · ')} · <button type="button" class="planner-link" onclick={onEditTarget}>Edit target</button></p>
    {/if}
    {#if conflict !== null}
        <p class="note warn"><Icon name="warning" size={15} />Ends before day {conflict}'s overnight · <button type="button" class="planner-link" onclick={onShowConflict}>Show</button></p>
    {/if}
    {#if expanded}
        <div class="body">
            <div class="stop"><span class="dot"></span><span>{start}</span><small>Start</small></div>
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
            {#if choosing}
                <div class="choose">
                    <div class="choose-head">
                        <button type="button" onclick={onShowEnd}><strong>Where to sleep</strong></button>
                        <small>Day {day.number} would be</small>
                    </div>
                    {#each candidates as candidate (candidate.place.id)}
                        <PlaceRow place={candidate.place} day={candidateDay(candidate)} selected={selectedId === candidate.place.id} onSelect={onSelectPlace} />
                    {/each}
                    <div class="choose-actions">
                        <button type="button" class="planner-link" onclick={onPick}>Pick another spot on the map</button>
                        {#if day.pinned}<button type="button" class="planner-link quiet" onclick={() => onChangeOvernight(false)}>Cancel</button>{/if}
                    </div>
                </div>
            {:else if day.pinned && !last}
                <button type="button" class="planner-link change" onclick={() => onChangeOvernight(true)}>Change overnight</button>
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
        background: var(--parchment);
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
        margin: 8px 0 0 36px;
    }
    .stop {
        display: flex;
        align-items: center;
        gap: 10px;
        width: calc(100% + 16px);
        min-height: 32px;
        margin: 0 -8px;
        padding: 0 8px;
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
        margin-top: 4px;
        font-size: 13px;
    }
    .change {
        margin-top: 4px;
        font-size: 13px;
    }
</style>
