<!-- A day of the day list: where it goes, its date and its figures, in the colour of its line on the map. Its foot is the
     night that ends it, as a suggestion or as the place that the rider set. -->
<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import { dayColor } from '../../lib/planner/day-colors';
    import { addDays, weekdayLabel } from '../../lib/planner/layers/data-layer';
    import { kindLabel, sleepIcon } from '../../lib/planner/search/presentation';
    import { stayNouns, stopCounts, suits } from '../../lib/planner/stops';
    import { duration, type Day, type Trip } from '../../lib/planner/editor';
    import type { NightChoices } from '../../lib/planner/night-stops.svelte';
    import { distanceIn, heightIn, heightUnit } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';

    let { trip, day, from, to, last, theme, ascentKnown, nights, conflict, profile, onOpen, onShowEnd, onShowConflict }: {
        trip: Trip;
        day: Day;
        /** Where the day starts and ends: a town, or the start or the finish of the trip. */
        from: string;
        to: string;
        /** The day ends at the finish and has no night. */
        last: boolean;
        theme: 'light' | 'dark';
        ascentKnown: boolean;
        nights: NightChoices;
        /** The day whose overnight this day ends before. */
        conflict: number | null;
        /** The climb of the day as paths in the box `view`. */
        profile?: { line: string; area: string; view: string };
        onOpen: () => void;
        /** Shows the place of a set night on the map. */
        onShowEnd: () => void;
        onShowConflict: () => void;
    } = $props();

    const pinned = $derived(day.pinned);
    const around = $derived(pinned ? undefined : nights.around(day.number));
    const found = $derived(nights.found(day.number));
    const kinds = $derived(nights.kinds(day.number));
    const towns = $derived(found?.state === 'ready' ? found.stops.filter(stop => suits(stop, kinds)).length : 0);
    const wanted = $derived(kinds.length === 1 ? stayNouns[kinds[0]] : 'town with a place to sleep');
    const status = $derived(found?.state === 'loading' ? 'Finding towns nearby…' : found?.state === 'error' ? 'Towns could not load'
        : towns ? `No ${wanted} near the day end · ${towns} ${towns === 1 ? 'town' : 'towns'} to choose from` : `No ${wanted} nearby`);
    const detail = $derived(!around ? status
        : around.place ? `${around.stop.name} · ${around.place.off < .15 ? 'on the route' : `${distanceIn(around.place.off * 1000).toFixed(1)} ${settings.units} off the route`}`
        : stopCounts(around.stop, kinds, true));
    const kind = $derived(pinned?.placeKind ? kindLabel(pinned.placeKind) : '');
</script>

<article class="day" style:--day-color={dayColor(day.number, theme)}>
    <button type="button" class="main" data-day={day.number} onclick={onOpen} aria-label={`Open day ${day.number}: ${from} to ${to}`}>
        <span class="badge">{day.number}</span>
        <span class="title">
            <strong>{from}<Icon name="arrow" size={14} />{to}</strong>
            {#if trip.startDate}<small>{weekdayLabel(addDays(trip.startDate, day.number - 1), true)}</small>{/if}
        </span>
        <span class="figures">
            <span><b>{distanceIn(day.distance * 1000).toFixed(1)}</b><small>{settings.units}</small></span>
            <span><Icon path="M4 18 10 9l4 5 6-9" size={14} />{ascentKnown ? Math.round(heightIn(day.ascent)) : '—'}<small>{heightUnit()}</small></span>
            <span><Icon name="clock" size={14} />{duration(day.hours)}</span>
            <Icon name="chevron" size={14} />
        </span>
        {#if profile}
            <svg class="profile" viewBox={profile.view} preserveAspectRatio="none" aria-hidden="true"><path class="area" d={profile.area} /><path class="line" d={profile.line} /></svg>
        {/if}
    </button>
    {#if conflict !== null}
        <p class="warn"><Icon name="warning" size={15} />Ends before day {conflict}'s overnight · <button type="button" class="planner-action" onclick={onShowConflict}>Show</button></p>
    {/if}
    {#if !last}
        <div class="night" class:set={!!pinned}>
            <span class="mark" aria-hidden="true"><Icon name={pinned ? sleepIcon(pinned.placeKind) : 'moon'} size={14} /></span>
            {#if pinned}
                <button type="button" class="text" onclick={onShowEnd}>
                    <strong>{pinned.label}</strong>
                    <small class="kind">{[kind, pinned.locality !== pinned.label && pinned.locality].filter(Boolean).join(' · ') || `Night ${day.number}`}</small>
                </button>
            {:else}
                <div class="text">
                    <strong>{around ? around.place?.place.label ?? `Sleep near ${around.stop.name}` : `Night ${day.number}`}</strong>
                    <small><span class="sr-only">Suggested. </span>{detail}</small>
                </div>
            {/if}
            <button type="button" class={pinned ? 'change' : 'planner-action'} data-night={day.number} aria-label={`${pinned ? 'Change' : 'Choose'} night ${day.number}`}
                onclick={() => nights.choose(day.number)}>{pinned ? 'Change' : 'Choose'}</button>
        </div>
    {/if}
</article>

<style>
    .day {
        overflow: hidden;
        border-radius: 12px;
        background: color-mix(in srgb, var(--day-color) 9%, var(--panel));
        color: var(--ink);
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
        padding: 12px;
        border-radius: 12px;
        text-align: left;
    }
    .main:hover { background: color-mix(in srgb, var(--day-color) 6%, transparent); }
    .badge {
        width: 24px;
        height: 24px;
        display: grid;
        place-items: center;
        border-radius: 50%;
        background: var(--day-color);
        color: var(--panel);
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
    .title :global(svg), .figures :global(svg) { color: color-mix(in srgb, var(--day-color) 70%, var(--ink)); }
    small {
        font-size: 13px;
        color: var(--ink-soft);
    }
    .title small { flex: none; }
    .figures {
        grid-column: 2;
        display: grid;
        grid-template-columns: 1.15fr 1fr 1fr 14px;
        align-items: baseline;
        font-size: 14px;
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    .figures b { font: 700 17px var(--sans); }
    .figures small { margin-left: 2px; }
    .figures span :global(svg) { margin: 0 3px -2px 0; }
    .figures > :global(svg) { align-self: center; color: var(--ink-faint); }
    /* The climb reaches the edges of the card. */
    .profile {
        grid-column: 1 / -1;
        display: block;
        width: calc(100% + 24px);
        height: 38px;
        margin: 0 -12px -12px;
    }
    .profile .area { fill: color-mix(in srgb, var(--day-color) 24%, transparent); }
    .profile .line {
        fill: none;
        stroke: var(--day-color);
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
        border-top: 1px solid color-mix(in srgb, var(--day-color) 22%, transparent);
    }
    .mark {
        width: 24px;
        height: 24px;
        flex: none;
        display: grid;
        place-items: center;
        box-sizing: border-box;
        border: 1.5px dashed color-mix(in srgb, var(--day-color) 70%, var(--ink));
        border-radius: 50%;
        color: color-mix(in srgb, var(--day-color) 70%, var(--ink));
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
    button.text:hover { background: color-mix(in srgb, var(--day-color) 8%, transparent); }
    .night strong, .night small {
        display: block;
        overflow-wrap: anywhere;
    }
    .night strong { font: 500 14px/1.3 var(--sans); }
    .set strong { font-weight: 600; }
    .kind::first-letter { text-transform: uppercase; }
    .night .planner-action { flex: none; min-height: 30px; font-size: 13px; }
    .change {
        flex: none;
        min-height: 30px;
        padding: 0 6px;
        border-radius: 6px;
        color: var(--ink-soft);
        font-size: 13px;
        font-weight: 600;
    }
    .change:hover { color: var(--ink); }
    .sr-only {
        position: absolute;
        width: 1px;
        height: 1px;
        overflow: hidden;
        clip-path: inset(50%);
        white-space: nowrap;
    }
</style>
