<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import StopCard from './StopCard.svelte';
    import StayChip from './StayChip.svelte';
    import { dayColor } from '../../lib/planner/day-colors';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import { daysAround, evenStop, stayNouns, stopCounts, stopKm, suits, type Stop } from '../../lib/planner/stops';
    import type { NightStops } from '../../lib/planner/night-stops.svelte';
    import type { Place, Stay, Trip } from '../../lib/planner/editor';
    import type { RoutingLine } from '../../lib/planner/routing';

    let { trip, line, night, labels, back, theme, found, window, stopId, shownId, date, elevationAt, kinds, onOpen, onEndHere, onStay, onSelectPlace, onPick, onRetry, onCancel, onKinds, onRelease }: {
        trip: Trip;
        line: RoutingLine;
        /** The riding day that ends at this night. */
        night: number;
        /** Calendar numbers of the day that ends and the day that starts here. */
        labels: [number, number];
        /** The text of the back row of a town card. */
        back?: string;
        theme: 'light' | 'dark';
        found: NightStops | undefined;
        /** The stretch searched, in km. */
        window: [number, number];
        stopId: string | null;
        /** The id of the place or point that the map shows. */
        shownId: string | null;
        /** The date of this night, when the trip has a start date. */
        date?: string;
        elevationAt: (km: number) => number | null;
        /** The kinds of place to sleep of this night. */
        kinds: Stay[];
        onOpen: (id: string | null) => void;
        onEndHere: (stop: Stop) => void;
        onStay: (place: Place) => void;
        /** Shows a place on the map; null shows none. */
        onSelectPlace: (place: Place | null) => void;
        onPick: () => void;
        onRetry: () => void;
        /** Closes the towns of an open day. */
        onCancel?: () => void;
        /** Sets the kinds of this night; null follows the trip. */
        onKinds: (kinds: Stay[] | null) => void;
        /** Makes a kept or moved night a suggestion again. */
        onRelease?: () => void;
    } = $props();

    const stops = $derived(found?.state === 'ready' ? found.stops.filter(stop => suits(stop, kinds)) : []);
    const even = $derived(evenStop(stops, trip, line, night));
    const rows = $derived(stops.map(stop => ({ stop, days: daysAround(trip, line, night, stop.km) })));
    // The longest pair of days fills the bar, so the bars compare across towns.
    const longest = $derived(Math.max(1e-9, ...rows.map(({ days }) => days[0].hours + days[1].hours)));
    const open = $derived(stops.find(stop => stop.id === stopId));
    const services = ['food', 'shop'] as const;
    // The town whose card closed, so its button takes the focus back.
    let returnTo: string | null = null;

    function duration(hours: number) {
        const minutes = Math.round(hours * 60);
        return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, '0')}m`;
    }
</script>

{#if open}
    <StopCard stop={open} {kinds} days={daysAround(trip, line, night, open.km)} {labels} {back} {shownId} {theme} {date} elevation={elevationAt(open.km)}
        onBack={() => { returnTo = open.id; onOpen(null); }} {onEndHere} {onStay} {onSelectPlace} />
{:else}
    <div class="picker">
        <div class="tools">
            <StayChip value={kinds} label={`Places to sleep on night ${labels[0]}`} onChange={onKinds} onReset={trip.nightStay?.[night] ? () => onKinds(null) : undefined} resetLabel="Same as the trip" />
            <div class="actions">
                <button type="button" class="planner-action" onclick={onPick}>Pick on the map</button>
                {#if onCancel}<button type="button" class="planner-action quiet" onclick={onCancel}>Cancel</button>{/if}
            </div>
        </div>
        {#if found?.state === 'ready'}
            {#if rows.length}
                <ul aria-label={`Towns for night ${labels[0]}`}>
                    {#each rows as { stop, days } (stop.id)}
                        <li>
                            <button type="button" class="town" onclick={() => onOpen(stop.id)} {@attach node => { if (stop.id === returnTo) { returnTo = null; node.focus(); } }}
                                aria-label={`${stop.name}, ${stopCounts(stop, kinds)}. Day ${labels[0]} ${duration(days[0].hours)}, day ${labels[1]} ${duration(days[1].hours)}${stop === even ? '. Even days' : ''}`}>
                                <strong>{stop.name}{#if stop === even}<em>Even days</em>{/if}</strong>
                                <small>{stopKm(stop, kinds)} · {stopCounts(stop, kinds, true)}
                                    <span class="services" aria-hidden="true">{#each services as service}{#if stop[service].length}<Icon path={placeCategories[service].icon} size={13} />{/if}{/each}</span>
                                </small>
                                <span class="bar" aria-hidden="true">
                                    <span style:width={`${days[0].hours / longest * 100}%`} style:background={dayColor(night, theme)}></span>
                                    <span style:width={`${days[1].hours / longest * 100}%`} style:background={dayColor(night + 1, theme)}></span>
                                </span>
                                <small class="hours" aria-hidden="true"><span>Day {labels[0]} · {duration(days[0].hours)}</span><span>Day {labels[1]} · {duration(days[1].hours)}</span></small>
                            </button>
                        </li>
                    {/each}
                </ul>
            {:else}
                <p class="state" role="status">No {kinds.length === 1 ? stayNouns[kinds[0]] : 'place to sleep'} between km {Math.round(window[0])} and {Math.round(window[1])}.</p>
            {/if}
        {:else if found?.state === 'error'}
            <p class="state" role="status">{found.message}</p>
            <button type="button" class="planner-action" onclick={onRetry}>Retry</button>
        {:else}
            <p class="state" role="status">Finding towns with somewhere to sleep…</p>
        {/if}
        {#if onRelease}<button type="button" class="planner-action release" onclick={onRelease}>Let the planner choose</button>{/if}
    </div>
{/if}

<style>
    .picker {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    .state {
        margin: 0;
        font-size: 13px;
        color: var(--ink-soft);
    }
    ul {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    li + li {
        border-top: 1px solid var(--line);
    }
    li {
        display: flex;
        align-items: center;
        gap: 8px;
    }
    .town {
        flex: 1;
        min-width: 0;
        margin-left: -8px;
        padding: 10px 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink);
        font: inherit;
        text-align: left;
        cursor: pointer;
    }
    .town:hover {
        background: var(--parchment-2);
    }
    button:focus-visible {
        outline: 2px solid var(--ink);
        outline-offset: 2px;
    }
    strong {
        display: flex;
        align-items: baseline;
        gap: 8px;
        font: 600 14px/1.3 var(--sans);
        overflow-wrap: anywhere;
    }
    em {
        flex: none;
        font: 600 12px var(--sans);
        font-style: normal;
        color: var(--forest);
    }
    small {
        display: block;
        margin-top: 2px;
        font-size: 13px;
        color: var(--ink-soft);
        font-variant-numeric: tabular-nums;
    }
    .services {
        display: inline-flex;
        gap: 6px;
        margin-left: 6px;
        vertical-align: -2px;
        color: var(--ink-faint);
    }
    .bar {
        display: flex;
        gap: 2px;
        height: 4px;
        margin-top: 6px;
    }
    .bar span {
        border-radius: 2px;
    }
    .bar span + span {
        opacity: .55;
    }
    .hours {
        display: flex;
        gap: 12px;
        font-size: 12px;
    }
    .tools {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 8px;
        padding-bottom: 10px;
        border-bottom: 1px solid var(--line);
    }
    .release {
        align-self: flex-start;
        font-size: 13px;
    }
    .actions {
        display: flex;
        gap: 8px;
        margin-left: auto;
        font-size: 13px;
    }
</style>
