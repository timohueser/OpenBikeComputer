<script lang="ts">
    import type { Snippet } from 'svelte';
    import Card from '../../lib/ui/Card.svelte';
    import Icon from '../planner/PlannerIcon.svelte';
    import type { RouteSummary } from '../../lib/library/records';
    import { distanceIn, formatDistance, formatHeight } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';
    import { duration, orderedRoutePoints, type RoutePoint, type Trip } from '../../lib/planner/editor';
    import type { Coordinate } from '../../lib/planner/geo';
    import { townName } from '../../lib/planner/point-names';
    import type { Climb, RouteFacts, Waypoint } from '../../lib/planner/route-facts';
    import { kindLabel } from '../../lib/planner/search/presentation';

    /** The left card of the Route page: the numbers of the stored summary, then the days, the climbs and the waypoints
     * of the calculated line, or the routing error. */
    let { summary, trip, facts, error, changed, children }: {
        summary: RouteSummary | null;
        trip: Trip;
        facts?: RouteFacts;
        error?: string;
        /** The calculated line differs from the stored summary. */
        changed: boolean;
        /** The buttons at the end of the card. */
        children: Snippet;
    } = $props();

    let climbsOpen = $state(false);
    // One place search for each climb top while the page lives.
    const towns = new Map<string, Promise<string | null>>();
    const town = (top: Coordinate) => {
        const key = top.join();
        if (!towns.has(key)) towns.set(key, townName(top));
        return towns.get(key)!;
    };

    const days = $derived(facts && facts.days.length > 1 ? facts.days : undefined);
    const ends = $derived(orderedRoutePoints(trip));
    const finish = $derived(trip.loop ? ends[0] : ends.at(-1));
    const longest = $derived(facts?.climbs.reduce<Climb | undefined>((best, climb) => !best || climb.to - climb.from > best.to - best.from ? climb : best, undefined));

    /** "km 19", or "mi 12" with miles. */
    const at = (km: number) => `${settings.units} ${Math.round(distanceIn(km * 1000))}`;
    const grade = (climb: Climb) => `${Math.round(climb.gradient)} %`;
    const about = (point: RoutePoint) => point.note ? `“${point.note}”` : point.placeKind ? kindLabel(point.placeKind) : '';
    const dayTitle = (from: string | undefined, to: string | undefined) => from && to ? `${from} to ${to}` : to ? `To ${to}` : from ? `From ${from}` : '';
</script>

{#snippet number(label: string, value: string)}
    <div><dt>{label}</dt><dd>{value}</dd></div>
{/snippet}

{#snippet pin(stop: Waypoint, detail: string)}
    <li class="stop"><span class="pin">{stop.number}</span><span><strong>{stop.point.label}</strong>{#if detail}<small>{detail}</small>{/if}</span></li>
{/snippet}

{#snippet climbRow(climb: Climb)}
    {@const numbers = `${at(climb.from)} · ${formatDistance((climb.to - climb.from) * 1000)} · ${grade(climb)}`}
    <li class="climb">
        {#await town(climb.top)}
            <strong>{numbers}</strong>
        {:then name}
            {#if name}<strong>To {name}</strong><small>{numbers}</small>{:else}<strong>{numbers}</strong>{/if}
        {/await}
    </li>
{/snippet}

<div class="facts">
    <Card>
        <dl class="numbers">
            {@render number('Length', summary ? formatDistance(summary.lengthM) : '–')}
            {@render number('Climb', summary ? formatHeight(summary.climbM) : '–')}
            {#if !days}
                {@render number('Time', summary ? duration(summary.seconds / 3600) : '–')}
                {@render number('Highest point', summary?.highestM != null ? formatHeight(summary.highestM) : '–')}
            {/if}
        </dl>
        {#if changed}<p class="note">Changed since the last save</p>{/if}

        {#if error}
            <p class="note" role="alert">{error}</p>
        {:else if !facts}
            <p class="note">Finding the climbs…</p>
        {:else if days}
            <h3>Days</h3>
            <ol class="days">
                {#each days as day (day.number)}
                    {#if day.rest}
                        <li class="day"><span class="chip">Day {day.number}</span><strong>{trip.restNames?.[day.restIndex!] || 'Rest day'}</strong></li>
                    {:else}
                        {@const from = day.ridingNumber === 1 ? ends[0]?.label : trip.points.find(p => p.kind === 'night' && p.night === day.ridingNumber - 1)?.label}
                        {@const last = day.ridingNumber === days.filter(d => !d.rest).length}
                        <li class="day">
                            <span class="chip">Day {day.number}</span>
                            <span>
                                <strong>{dayTitle(from, last ? finish?.label : day.pinned?.label)}</strong>
                                <small>{formatDistance(day.distance * 1000)} · {formatHeight(day.ascent)} up · {duration(day.hours)}</small>
                            </span>
                        </li>
                        {#each day.stops as stop (stop.point.id)}{@render pin(stop, about(stop.point))}{/each}
                        {#if !last}
                            <li class="day"><span class="chip">Night</span><span>{day.pinned?.label ?? 'Not chosen'}</span></li>
                        {/if}
                    {/if}
                {/each}
            </ol>
            {#if longest}
                <button type="button" class="more" aria-expanded={climbsOpen} onclick={() => climbsOpen = !climbsOpen}>
                    <span><strong>{facts.climbs.length} {facts.climbs.length === 1 ? 'climb' : 'climbs'}</strong> · the longest: {formatDistance((longest.to - longest.from) * 1000)} at {grade(longest)}</span>
                    <Icon name="chevron" size={14} />
                </button>
                {#if climbsOpen}<ul class="rows">{#each facts.climbs as climb (climb.from)}{@render climbRow(climb)}{/each}</ul>{/if}
            {/if}
        {:else}
            {#if facts.climbs.length}
                <h3>Climbs</h3>
                <ul class="rows">{#each facts.climbs as climb (climb.from)}{@render climbRow(climb)}{/each}</ul>
            {/if}
            {#if facts.stops.length}
                <h3>Waypoints</h3>
                <ol class="rows">{#each facts.stops as stop (stop.point.id)}{@render pin(stop, [about(stop.point), at(stop.km)].filter(Boolean).join(' · '))}{/each}</ol>
            {/if}
        {/if}

        <div class="actions">{@render children()}</div>
    </Card>
</div>

<style>
    .facts {
        min-height: 0;
    }
    .facts > :global(section) {
        display: flex;
        flex-direction: column;
        height: 100%;
        overflow: auto;
    }
    .numbers {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 14px 12px;
        margin: 0;
    }
    dt, h3 {
        color: var(--ink-soft);
        font: 600 11px/1.3 var(--sans);
        letter-spacing: .06em;
        text-transform: uppercase;
    }
    dd {
        margin: 2px 0 0;
        font: 600 21px/1.2 var(--sans);
    }
    h3 {
        margin: 20px 0 0;
        padding-bottom: 6px;
        border-bottom: 1px solid var(--line);
    }
    .note {
        margin: 12px 0 0;
        color: var(--ink-soft);
    }
    ol, ul {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    li {
        display: flex;
        gap: 10px;
        align-items: flex-start;
        padding: 9px 0;
    }
    .rows > li, .day {
        border-bottom: 1px solid var(--line);
    }
    .climb {
        flex-direction: column;
        gap: 2px;
    }
    li > span:not(.chip, .pin) {
        display: flex;
        flex-direction: column;
        gap: 2px;
        min-width: 0;
    }
    small {
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    .stop {
        padding: 5px 0 5px 12px;
        border: 0;
    }
    .stop strong {
        font-weight: 400;
    }
    .pin {
        display: grid;
        flex: none;
        place-items: center;
        width: 20px;
        height: 20px;
        border-radius: 50%;
        background: var(--rust);
        color: var(--cream);
        font: 700 11px/1 var(--sans);
    }
    .chip {
        flex: none;
        padding: 2px 6px;
        border-radius: 4px;
        background: var(--parchment-2);
        font-size: 12px;
    }
    .more {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        width: 100%;
        padding: 10px 0;
        border: 0;
        border-bottom: 1px solid var(--line);
        background: none;
        color: var(--ink);
        text-align: left;
    }
    .more[aria-expanded="true"] :global(svg) {
        transform: rotate(90deg);
    }
    .actions {
        display: flex;
        justify-content: space-between;
        gap: 8px;
        margin-top: auto;
        padding-top: 16px;
    }
</style>
