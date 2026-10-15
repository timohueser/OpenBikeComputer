<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import StopCard from './StopCard.svelte';
    import { dayColor } from '../../lib/planner/day-colors';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import { daysAround, evenStop, stopCounts, type Stop } from '../../lib/planner/stops';
    import type { NightStops } from '../../lib/planner/night-stops.svelte';
    import type { Place, Trip } from '../../lib/planner/editor';
    import type { RoutingLine } from '../../lib/planner/routing';

    let { trip, line, night, labels, theme, found, window, stopId, date, elevationAt, onOpen, onEndHere, onStay, onSelectPlace, onPick, onRetry, onCancel }: {
        trip: Trip;
        line: RoutingLine;
        /** The riding day that ends at this night. */
        night: number;
        /** Calendar numbers of the day that ends and the day that starts here. */
        labels: [number, number];
        theme: 'light' | 'dark';
        found: NightStops | undefined;
        /** The stretch searched, in km. */
        window: [number, number];
        stopId: string | null;
        /** The date of this night, when the trip has a start date. */
        date?: string;
        elevationAt: (km: number) => number | null;
        onOpen: (id: string | null) => void;
        onEndHere: (stop: Stop) => void;
        onStay: (place: Place) => void;
        onSelectPlace: (place: Place) => void;
        onPick: () => void;
        onRetry: () => void;
        onCancel?: () => void;
    } = $props();

    const stops = $derived(found?.state === 'ready' ? found.stops : []);
    const even = $derived(evenStop(stops, trip, line, night));
    const rows = $derived(stops.map(stop => ({ stop, days: daysAround(trip, line, night, stop.km) })));
    // The longest pair of days fills the bar, so the bars compare across towns.
    const longest = $derived(Math.max(1e-9, ...rows.map(({ days }) => days[0].hours + days[1].hours)));
    const open = $derived(stops.find(stop => stop.id === stopId));
    const services = ['food', 'shop', 'water'] as const;

    function duration(hours: number) {
        const minutes = Math.round(hours * 60);
        return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, '0')}m`;
    }
</script>

{#if open}
    <StopCard stop={open} days={daysAround(trip, line, night, open.km)} {labels} {theme} {date} elevation={elevationAt(open.km)}
        onBack={() => onOpen(null)} {onEndHere} {onStay} {onSelectPlace} />
{:else}
    <div class="picker">
        <p class="lead">Towns with somewhere to sleep between km {Math.round(window[0])} and {Math.round(window[1])}. Each bar shows day {labels[0]} and day {labels[1]}.</p>
        {#if found?.state === 'ready'}
            {#if rows.length}
                <ul aria-label={`Towns for night ${labels[0]}`}>
                    {#each rows as { stop, days } (stop.id)}
                        <li>
                            <button type="button" class="town" onclick={() => onOpen(stop.id)}
                                aria-label={`${stop.name}, ${stopCounts(stop)}. Day ${labels[0]} ${duration(days[0].hours)}, day ${labels[1]} ${duration(days[1].hours)}${stop === even ? '. Even days' : ''}`}>
                                <span class="name">
                                    <strong>{stop.name}{#if stop === even}<em>Even days</em>{/if}</strong>
                                    <small>km {Math.round(stop.from)}{Math.round(stop.to) !== Math.round(stop.from) ? `–${Math.round(stop.to)}` : ''} · {stopCounts(stop)}</small>
                                    <span class="services" aria-hidden="true">
                                        {#each services as service}{#if stop[service].length}<Icon path={placeCategories[service].icon} size={13} />{/if}{/each}
                                    </span>
                                </span>
                                <span class="balance" aria-hidden="true">
                                    <span class="hours"><span style:color={dayColor(night, theme)}>{duration(days[0].hours)}</span><span style:color={dayColor(night + 1, theme)}>{duration(days[1].hours)}</span></span>
                                    <span class="bar">
                                        <span style:width={`${days[0].hours / longest * 100}%`} style:background={dayColor(night, theme)}></span>
                                        <span style:width={`${days[1].hours / longest * 100}%`} style:background={dayColor(night + 1, theme)}></span>
                                    </span>
                                </span>
                                <Icon name="chevron" size={13} />
                            </button>
                        </li>
                    {/each}
                </ul>
            {:else}
                <p class="state" role="status">No mapped place to sleep between km {Math.round(window[0])} and {Math.round(window[1])}. Pick a spot on the map.</p>
            {/if}
        {:else if found?.state === 'error'}
            <p class="state" role="status">{found.message}</p>
            <button type="button" class="planner-action" onclick={onRetry}>Retry</button>
        {:else}
            <p class="state" role="status">Finding towns with somewhere to sleep…</p>
        {/if}
        <div class="actions">
            <button type="button" class="planner-action" onclick={onPick}>Pick a spot on the map</button>
            {#if onCancel}<button type="button" class="planner-action quiet" onclick={onCancel}>Cancel</button>{/if}
        </div>
    </div>
{/if}

<style>
    .picker {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    .lead,
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
    .town {
        display: flex;
        align-items: center;
        gap: 12px;
        width: calc(100% + 16px);
        margin: 0 -8px;
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
    .town:focus-visible {
        outline: 2px solid var(--ink);
        outline-offset: 2px;
    }
    .town > :global(svg) {
        flex: none;
        color: var(--ink-faint);
    }
    .name {
        flex: 1;
        min-width: 0;
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
        display: flex;
        gap: 6px;
        margin-top: 4px;
        color: var(--ink-faint);
    }
    .balance {
        flex: none;
        width: 112px;
        display: flex;
        flex-direction: column;
        gap: 4px;
    }
    .hours {
        display: flex;
        justify-content: space-between;
        font: 600 12px var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .bar {
        display: flex;
        gap: 2px;
        height: 4px;
    }
    .bar span {
        border-radius: 2px;
    }
    .bar span + span {
        opacity: .55;
    }
    .actions {
        display: flex;
        justify-content: space-between;
        gap: 8px;
        font-size: 13px;
    }
</style>
