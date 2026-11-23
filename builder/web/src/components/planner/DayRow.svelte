<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { Attachment } from 'svelte/attachments';
    import Icon from '../../lib/ui/Icon.svelte';
    import RouteStats from './RouteStats.svelte';
    import { dayColor } from '../../lib/planner/day-colors';
    import { addDays, weekdayLabel } from '../../lib/planner/layers/data-layer';
    import { sleepIcon } from '../../lib/planner/search/presentation';
    import { dayStops, planView, type RoutePoint, type Trip, type Day } from '../../lib/planner/editor';
    import type { RoutingLine } from '../../lib/planner/routing';
    import { distanceIn } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';

    let {
        trip, line, day, days, theme, choosing, conflict, selectedId, revealId, hoveredId = null, onHover,
        onInspect, onShowEnd, onChange, onShowConflict, picker, suggested = {},
    }: {
        trip: Trip;
        line?: RoutingLine;
        day: Day;
        days: Day[];
        theme: 'light' | 'dark';
        /** The towns to sleep in show for this day. */
        choosing: boolean;
        /** The day whose overnight this day ends before. */
        conflict: number | null;
        selectedId: string | null;
        hoveredId?: string | null;
        onHover?: (id: string | null) => void;
        /** The point whose row scrolls into view and lights up for a moment. */
        revealId: string | null;
        onInspect: (point: RoutePoint) => void;
        onShowEnd: () => void;
        /** Opens the towns again for a pinned night. */
        onChange: () => void;
        onShowConflict: () => void;
        picker?: Snippet;
        /** The suggested night of each day by number, such as "Around Engen", where no night is pinned. */
        suggested?: Record<number, string>;
    } = $props();

    const last = $derived(day.number === days.length);
    const previous = $derived(days[day.number - 2]);
    const start = $derived(day.number === 1 ? trip.points.find(p => p.kind === 'start')!.label : previous?.pinned?.label ?? suggested[day.number - 1] ?? `Day ${day.number - 1} overnight`);
    const view = $derived(planView(trip, line));
    const finish = $derived(view.stops.at(-1)!.point);
    const end = $derived(last ? finish.label : day.pinned?.label ?? suggested[day.number] ?? 'Overnight to choose');
    const endId = $derived(last ? finish.id : day.pinned?.id ?? null);
    const ascentKnown = $derived(line?.unknownElevationKm === 0);
    const stops = $derived(dayStops(trip, line, day));
    const title = $derived(`Day ${day.number}${trip.startDate ? ` · ${weekdayLabel(addDays(trip.startDate, day.number - 1))}` : ''}`);
    const stopKinds: Record<string, { label: string; icon: string }> = {
        waypoint: { label: 'Visit', icon: 'flag' },
        detour: { label: 'Visit', icon: 'flag' },
        pass: { label: 'Pass', icon: 'pin' },
        marker: { label: 'Marker', icon: 'pin' },
    };

    function reveal(id: string): Attachment {
        return (node) => {
            if (revealId === id) node.scrollIntoView({ block: 'nearest', behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' });
        };
    }
</script>

<section class="day" style:--day-color={dayColor(day.number, theme)}>
    <header class="detail-heading">
        <span class="badge">{day.number}</span>
        <div><small>{title}</small><h2>{start} → {end}</h2></div>
    </header>
    <RouteStats distance={day.distance} ascent={ascentKnown ? day.ascent : null} descent={ascentKnown ? day.descent : null} walking={trip.bike === 'hiking'} hours={line ? day.hours : null} />
    {#if conflict !== null}
        <p class="note warn"><Icon name="warning" size={15} />Ends before day {conflict}'s overnight · <button type="button" class="planner-action" onclick={onShowConflict}>Show</button></p>
    {/if}
    <div class="body">
        <h3>Stops along the day</h3>
        {#if stops.length}<div class="stop"><span class="dot"></span><span>{start}</span><small>0 {settings.units}</small></div>{/if}
        {#each stops as { point, km } (point.id)}
            <button type="button" class="stop" class:chosen={selectedId === point.id || hoveredId === point.id} class:flash={revealId === point.id} onmouseenter={() => onHover?.(point.id)} onmouseleave={() => onHover?.(null)} onfocus={() => onHover?.(point.id)} onblur={() => onHover?.(null)} onclick={() => onInspect(point)} {@attach reveal(point.id)}>
                <Icon name={stopKinds[point.kind].icon} size={15} /><span>{point.label}</span>
                <small>{stopKinds[point.kind].label} · at {distanceIn(km * 1000).toFixed(1)} {settings.units}</small>
            </button>
        {/each}
        {#if last || day.pinned}
            <button type="button" class="stop end" class:chosen={hoveredId === endId} onmouseenter={() => onHover?.(endId)} onmouseleave={() => onHover?.(null)} onfocus={() => onHover?.(endId)} onblur={() => onHover?.(null)} class:flash={revealId === endId} onclick={onShowEnd} {@attach reveal(endId!)}>
                <Icon name={last ? 'flag' : sleepIcon(day.pinned?.placeKind)} size={15} /><span>{end}</span><small>{last ? 'Finish' : 'Pinned'}</small>
            </button>
        {/if}
        {#if stops.length && !last && !day.pinned}
            <button type="button" class="stop end" onclick={onShowEnd}><Icon name="camp" size={15} /><span>Suggested day end<small>Choose an overnight below</small></span><small>{distanceIn(day.distance * 1000).toFixed(1)} {settings.units}</small></button>
        {/if}
        {#if !stops.length}<p class="help">Click the map to add a stop.</p>{/if}
        {#if choosing && !last}
            <div class="choose">
                <div class="choose-head">
                    <h3>Where to sleep</h3>
                    <button type="button" class="planner-action" onclick={onShowEnd}>Show area</button>
                </div>
                {@render picker?.()}
            </div>
        {:else if day.pinned && !last}
            <button type="button" class="planner-action change" onclick={onChange}>Change overnight</button>
        {/if}
    </div>
</section>

<style>
    .day {
        padding: 4px 8px 12px;
        border-radius: 8px;
    }
    .detail-heading { display: flex; align-items: center; gap: 10px; }
    .detail-heading small { display: block; font-size: 13px; color: var(--ink-soft); }
    h2 { margin: 0; font: 600 17px/1.35 var(--sans); overflow-wrap: anywhere; }
    h3 { margin: 0; font: 600 14px var(--sans); }
    .help { margin: 6px 0 12px; font-size: 13px; color: var(--ink-soft); }

    button {
        border: 0;
        background: none;
        color: inherit;
        font: inherit;
        cursor: pointer;
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
    .note {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 4px;
        margin: 4px 0 0;
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
        margin-bottom: 6px;
    }
    .change {
        margin-top: 4px;
        font-size: 13px;
    }
</style>
