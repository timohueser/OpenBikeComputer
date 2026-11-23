<!-- A day of the day list: where it goes, its date, its figures and its climb, with the colour of its line on the map.
     Its foot is the night that ends it, as a suggestion or as the place that the rider set. The card of the open day has
     the descent too, and no foot. -->
<script lang="ts" module>
    import type { Day } from '../../lib/planner/editor';
    import type { NightChoices } from '../../lib/planner/night-stops.svelte';
    import { kindLabel, sleepIcon } from '../../lib/planner/search/presentation';
    import { stayNouns, stopCounts, suits } from '../../lib/planner/stops';
    import { distanceIn } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';

    /** The night that ends `day`, as the icon of its mark, its name and its sub line: the place that the rider set, else
     * the suggestion. */
    export function nightText(day: Day, nights: NightChoices): { icon: string; label: string; sub: string } {
        const pinned = day.pinned, kinds = nights.kinds(day.number);
        if (pinned) return { icon: sleepIcon(pinned.placeKind), label: pinned.label,
            sub: [pinned.placeKind && kindLabel(pinned.placeKind), pinned.locality !== pinned.label && pinned.locality].filter(Boolean).join(' · ') || `Night ${day.number}` };
        const around = nights.around(day.number);
        if (around) return { icon: 'moon', label: around.place?.place.label ?? `Sleep near ${around.stop.name}`, sub: !around.place ? stopCounts(around.stop, kinds, true)
            : `${around.stop.name} · ${around.place.off < .15 ? 'on the route' : `${distanceIn(around.place.off * 1000).toFixed(1)} ${settings.units} off the route`}` };
        const found = nights.found(day.number);
        const towns = found?.state === 'ready' ? found.stops.filter(stop => suits(stop, kinds)).length : 0;
        const wanted = kinds.length === 1 ? stayNouns[kinds[0]] : 'town with a place to sleep';
        return { icon: 'moon', label: `Night ${day.number}`, sub: found?.state === 'loading' ? 'Finding towns nearby…' : found?.state === 'error' ? 'Towns could not load'
            : towns ? `No ${wanted} near the day end · ${towns} ${towns === 1 ? 'town' : 'towns'} to choose from` : `No ${wanted} nearby` };
    }
</script>

<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import NightButton from './NightButton.svelte';
    import { dayColor } from '../../lib/planner/day-colors';
    import { addDays, weekdayLabel } from '../../lib/planner/layers/data-layer';
    import { duration, type Trip } from '../../lib/planner/editor';
    import { heightIn, heightUnit } from '../../lib/settings/format';

    let { trip, day, from, to, foot = false, flash = false, theme, ascentKnown, nights, conflict, profile, onOpen, onShowEnd, onShowConflict }: {
        trip: Trip;
        day: Day;
        /** Where the day starts and ends: a town, or the start or the finish of the trip. */
        from: string;
        to: string;
        /** The card shows the night that ends the day. The last day has none, and an open night stands under its card. */
        foot?: boolean;
        /** The night lights up for a moment after the rider set it. */
        flash?: boolean;
        theme: 'light' | 'dark';
        ascentKnown: boolean;
        nights: NightChoices;
        /** The day whose overnight this day ends before. */
        conflict: number | null;
        /** The climb of the day as paths in the box `view`. */
        profile?: { line: string; area: string; view: string };
        /** Opens the day. The card of the open day has none: it is not a button, and it shows the descent. */
        onOpen?: () => void;
        /** Shows the place of a set night on the map. */
        onShowEnd?: () => void;
        onShowConflict: () => void;
    } = $props();

    const night = $derived(nightText(day, nights));
    const height = (metres: number) => ascentKnown ? Math.round(heightIn(metres)) : '—';
</script>

{#snippet main()}
    <span class="badge">{day.number}</span>
    <span class="title">
        <strong>{from}<Icon name="arrow" size={14} />{to}</strong>
        {#if trip.startDate}<small>{weekdayLabel(addDays(trip.startDate, day.number - 1), true)}</small>{/if}
    </span>
    <span class="figures" class:open={!onOpen}>
        <span><b>{distanceIn(day.distance * 1000).toFixed(1)}</b><small>{settings.units}</small></span>
        <span><Icon path="M4 18 10 9l4 5 6-9" size={14} /><b>{height(day.ascent)}</b><small>{heightUnit()}</small></span>
        {#if onOpen}
            <span><Icon name="clock" size={14} /><b>{duration(day.hours)}</b></span>
            <Icon name="chevron" size={14} />
        {:else}
            <span><Icon path="M4 6 10 15l4-5 6 9" size={14} /><b>{height(day.descent)}</b><small>{heightUnit()}</small></span>
            <span><Icon name="clock" size={14} /><b>{duration(day.hours)}</b></span>
        {/if}
    </span>
    {#if profile}
        <svg class="profile" viewBox={profile.view} preserveAspectRatio="none" aria-hidden="true"><path class="area" d={profile.area} /><path class="line" d={profile.line} /></svg>
    {/if}
{/snippet}

<article class="day" style:--day-color={dayColor(day.number, theme)} style:--day-deep={dayColor(day.number, 'light')}>
    {#if onOpen}
        <button type="button" class="main" data-day={day.number} onclick={onOpen} aria-label={`Open day ${day.number}: ${from} to ${to}`}>{@render main()}</button>
    {:else}
        <div class="main">{@render main()}</div>
    {/if}
    {#if conflict !== null}
        <p class="warn"><Icon name="warning" size={15} />Ends before day {conflict}'s overnight · <button type="button" class="planner-action" onclick={onShowConflict}>Show</button></p>
    {/if}
    {#if foot}
        <div class="night" class:set={!!day.pinned} class:flash>
            <span class="mark" aria-hidden="true"><Icon name={night.icon} size={14} /></span>
            {#if day.pinned}
                <button type="button" class="text" onclick={onShowEnd}>
                    <strong>{night.label}</strong>
                    <small class="kind">{night.sub}</small>
                </button>
            {:else}
                <div class="text">
                    <strong>{night.label}</strong>
                    <small><span class="sr-only">Suggested. </span>{night.sub}</small>
                </div>
            {/if}
            <NightButton night={day.number} set={!!day.pinned} onclick={() => nights.choose(day.number)} />
        </div>
    {/if}
</article>

<style>
    /* The number of the day carries the colour of the light theme, which is deep enough for white text in both themes.
       The climb carries the colour of the line on the map; the dark theme mutes it. */
    .day {
        --climb: var(--day-color);
        --climb-fill: color-mix(in srgb, var(--day-color) 24%, transparent);
        overflow: hidden;
        border-radius: 12px;
        background: color-mix(in srgb, var(--ink) 7%, var(--panel));
        color: var(--ink);
    }
    :global([data-theme="dark"]) .day {
        --climb: color-mix(in srgb, var(--day-color) 62%, var(--panel));
        --climb-fill: color-mix(in srgb, var(--day-color) 16%, transparent);
    }
    button {
        border: 0;
        background: none;
        color: inherit;
        font: inherit;
        cursor: pointer;
    }
    button:focus-visible {
        outline: 2px solid var(--ink);
        outline-offset: -2px;
    }
    .main {
        display: grid;
        grid-template-columns: 24px minmax(0, 1fr);
        gap: 8px 10px;
        width: 100%;
        box-sizing: border-box;
        padding: 12px;
        border-radius: 12px;
        text-align: left;
    }
    button.main:hover { background: color-mix(in srgb, var(--ink) 4%, transparent); }
    .badge {
        width: 24px;
        height: 24px;
        display: grid;
        place-items: center;
        border-radius: 50%;
        background: var(--day-deep);
        color: #fff;
        font: 700 13px var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .title {
        display: flex;
        align-items: baseline;
        gap: 8px;
        min-width: 0;
        padding-top: 2px;
    }
    .title strong {
        flex: 1;
        min-width: 0;
        font: 600 15px/1.3 var(--sans);
        overflow-wrap: anywhere;
    }
    .title strong :global(svg) { margin: 0 4px -2px; }
    .title :global(svg), .figures :global(svg) { color: var(--ink-faint); }
    small {
        font-size: 13px;
        color: var(--ink-soft);
    }
    .title small { flex: none; }
    /* Each figure has its column, so the figures line up from card to card. */
    .figures {
        grid-column: 2;
        display: grid;
        grid-template-columns: 1.15fr 1fr 1fr 14px;
        align-items: baseline;
        white-space: nowrap;
    }
    /* Four figures share the line: the gaps between them take what is left. */
    .figures.open { grid-template-columns: repeat(4, auto); justify-content: space-between; }
    /* `font` resets the tabular figures, so they follow it. */
    .figures b { font: 600 15px var(--sans); font-variant-numeric: tabular-nums; }
    .figures small { margin-left: 2px; }
    .figures span :global(svg) { margin: 0 3px -2px 0; }
    .figures > :global(svg) { align-self: center; }
    /* The climb reaches the edges of the card. */
    .profile {
        grid-column: 1 / -1;
        display: block;
        width: calc(100% + 24px);
        height: 38px;
        margin: 0 -12px -12px;
    }
    .profile .area { fill: var(--climb-fill); }
    .profile .line {
        fill: none;
        stroke: var(--climb);
        stroke-width: 2;
        stroke-linejoin: round;
        vector-effect: non-scaling-stroke;
    }
    .warn {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 4px;
        margin: 0;
        padding: 0 12px 10px 46px;
        font-size: 13px;
        color: var(--coral);
    }
    .warn :global(svg) { margin-right: 2px; }
    .night {
        display: flex;
        align-items: center;
        gap: 10px;
        min-height: 48px;
        padding: 6px 12px;
        border-top: 1px solid var(--line);
    }
    .mark {
        width: 24px;
        height: 24px;
        flex: none;
        display: grid;
        place-items: center;
        box-sizing: border-box;
        border: 1.5px dashed var(--ink-soft);
        border-radius: 50%;
        color: var(--ink-soft);
    }
    .set .mark {
        border: 0;
        background: var(--ink);
        color: var(--panel);
    }
    .text {
        flex: 1;
        min-width: 0;
    }
    button.text {
        margin: -4px 0 -4px -6px;
        padding: 4px 6px;
        border-radius: 6px;
        text-align: left;
    }
    button.text:hover { background: color-mix(in srgb, var(--ink) 5%, transparent); }
    .night strong, .night small {
        display: block;
        overflow-wrap: anywhere;
    }
    .night strong { font: 500 14px/1.3 var(--sans); }
    .set strong { font-weight: 600; }
    .kind::first-letter { text-transform: uppercase; }
    .flash { animation: flash 1.2s ease-out; }
    @keyframes flash {
        from { background: color-mix(in srgb, var(--amber) 40%, transparent); }
        to { background: transparent; }
    }
    @media (prefers-reduced-motion: reduce) {
        .flash {
            animation: none;
            background: color-mix(in srgb, var(--amber) 40%, transparent);
        }
    }
    .sr-only {
        position: absolute;
        width: 1px;
        height: 1px;
        overflow: hidden;
        clip-path: inset(50%);
        white-space: nowrap;
    }
</style>
