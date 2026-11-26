<!-- The route line of the open day in its colour: where the day starts, its stops, and where it ends. -->
<script lang="ts">
    import { nightText } from './DayCard.svelte';
    import NightButton from './NightButton.svelte';
    import RouteRow, { hint, stopSign } from './RouteRow.svelte';
    import { dayColor } from '../../lib/planner/day-colors';
    import { dayStops, orderedRoutePoints, type Day, type RoutePoint, type Trip } from '../../lib/planner/editor';
    import type { NightChoices } from '../../lib/planner/night-stops.svelte';
    import type { RoutingLine } from '../../lib/planner/routing';
    import { sleepIcon } from '../../lib/planner/search/presentation';
    import { formatAlong } from '../../lib/settings/format';

    let { trip, line, day, days, from, theme, nights, revealId, hoveredId = null, onHover, onInspect, onShowEnd }: {
        trip: Trip;
        line?: RoutingLine;
        day: Day;
        days: Day[];
        /** The town of the night before the day. */
        from: string;
        theme: 'light' | 'dark';
        nights: NightChoices;
        /** The point whose row scrolls into view and lights up for a moment. */
        revealId: string | null;
        hoveredId?: string | null;
        onHover?: (id: string | null) => void;
        onInspect: (point: RoutePoint) => void;
        /** Shows the finish, or the place of a set night, on the map. */
        onShowEnd: () => void;
    } = $props();

    // A route that returns ends at its start.
    const route = $derived(orderedRoutePoints(trip));
    const before = $derived(days[day.number - 2]?.pinned);
    const night = $derived(nightText(day, nights));
    /** The row of a point: a click shows the point on the map, and the pointer or the focus on the row lights its pin. */
    const row = (point: RoutePoint, onclick = () => onInspect(point)) => ({ label: point.label, lit: hoveredId === point.id, reveal: revealId === point.id, onclick,
        onHover: (on: boolean) => onHover?.(on ? point.id : null) });
</script>

<ol style:--route={dayColor(day.number, theme)}>
    {#if day.number === 1}
        <RouteRow sign="start" along="Start" {...row(route[0])} />
    {:else if before}
        <RouteRow sign="night" icon={sleepIcon(before.placeKind)} label={before.label} sub={`Night ${day.number - 1}`} />
    {:else}
        <RouteRow sign="suggested" label={from} sub={`Night ${day.number - 1} · suggested`} />
    {/if}
    {#each dayStops(trip, line, day) as { point, km } (point.id)}
        <RouteRow sign={stopSign(point)} along={formatAlong(km * 1000)} {...row(point)} />
    {:else}
        <RouteRow {...hint} />
    {/each}
    {#if day.number === days.length}
        <RouteRow sign="finish" along={formatAlong(day.distance * 1000)} {...row(route.at(-1)!, onShowEnd)} />
    {:else}
        <RouteRow sign={day.pinned ? 'night' : 'suggested'} {...night} {...day.pinned && row(day.pinned, onShowEnd)}>
            <NightButton night={day.number} set={!!day.pinned} onclick={() => nights.choose(day.number)} />
        </RouteRow>
    {/if}
</ol>

<style>
    ol {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    /* The button ends where the distances of the stops end. */
    ol :global([data-night]) { margin-right: 4px; }
</style>
