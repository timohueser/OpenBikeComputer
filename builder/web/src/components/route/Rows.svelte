<script lang="ts">
    import { untrack } from 'svelte';
    import Card from '../../lib/ui/Card.svelte';
    import Segmented from '../../lib/ui/Segmented.svelte';
    import OpeningHours from '../planner/OpeningHours.svelte';
    import ElevationRow from './ElevationRow.svelte';
    import Lanes from './Lanes.svelte';
    import type { RouteSummary } from '../../lib/library/records';
    import { lineCoordinates } from '../../lib/library/summary';
    import { phoneLink, phoneNumbers, websiteLink } from '../../lib/planner/contact-links';
    import { orderedRoutePoints, planView, type Place, type RoutePoint, type Trip } from '../../lib/planner/editor';
    import { cumulative, nearestProgress, routeSlice } from '../../lib/planner/geo';
    import { profileGrades } from '../../lib/planner/grade-data';
    import { config } from '../../lib/planner/map-data';
    import { corridorPlaces } from '../../lib/planner/place-index';
    import { categoryIds, placeCategories, type PlaceCategory } from '../../lib/planner/poi-kinds';
    import { profileHeightAt, profileSamples, sampleIndex } from '../../lib/planner/profile-data';
    import type { Pin, RouteFacts } from '../../lib/planner/route-facts';
    import type { RoutingLine, Surface } from '../../lib/planner/routing';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import { placeDetails, type SearchPlace } from '../../lib/planner/search/types';
    import { surfaceBarColors, surfaceRuns, surfaceWindow } from '../../lib/planner/surface-data';
    import { distanceIn, formatDistance, formatHeight, formatPercent } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';

    /** The bottom card of the Route page: the waypoints, the heights, the surface and the places along the route or one
     * day of it, on one axis. Shares are 0 to 1 of the route. Without the calculated line, the rows that the summary
     * holds show. */
    let { summary, trip, line, facts, pins, mark, onpoint }: {
        summary: RouteSummary;
        trip: Trip;
        line?: RoutingLine;
        facts?: RouteFacts;
        pins: Pin[];
        mark?: number;
        onpoint: (share: number | null) => void;
    } = $props();

    /** Places load within PLACE_M of the shown stretch, for at most PLACES_KM of it. Icons closer than JOIN_PX join into one.
     * A tick label is about TICK_CHAR_PX wide for each character, and keeps TICK_GAP_PX from the next one. */
    const PLACE_M = 1000, PLACES_KM = 300, JOIN_PX = 12, TICK_CHAR_PX = 7, TICK_GAP_PX = 12, PANEL_PX = 260;
    const searchable = /^(?:[nwr]|Q)[1-9]\d*$/;

    let day = $state('all'), category = $state<PlaceCategory | ''>(''), open = $state<number>(), shift = $state(0);
    let placeLane = $state<HTMLElement>(), width = $state(0), seen = $state(false);
    let places = $state.raw<{ found?: Place[]; failed?: boolean }>({});
    let details = $state.raw<Record<string, SearchPlace | null>>({});

    const coordinates = $derived(lineCoordinates(summary));
    const profile = $derived(line ?? { coordinates, elevation: summary.heights });
    const samples = $derived(profileSamples(profile));
    const grades = $derived(profileGrades(samples, cumulative(profile.coordinates).at(-1) ?? 0));
    const totalM = $derived(line ? planView(trip, line).total * 1000 : summary.lengthM);
    const days = $derived(facts?.days.filter(item => !item.rest) ?? []);
    const shown = $derived(days.find(item => String(item.number) === day) ?? { from: 0, to: 1 });
    const span = $derived(Math.max(1e-6, shown.to - shown.from));
    const x = (share: number) => (share - shown.from) / span;
    const inside = (share: number) => share >= shown.from && share <= shown.to;
    // The start tick names the night at the start of the shown stretch.
    const shownPins = $derived(pins.filter(pin => pin.number ? inside(pin.share) : pin.share > shown.from && pin.share <= shown.to));
    // The shares of a day belong to the calculated line.
    const slice = $derived(routeSlice(profile.coordinates, shown.from, shown.to));
    const nights = $derived(pins.filter(pin => !pin.number).map(pin => pin.share));
    const runs = $derived(line ? surfaceWindow(surfaceRuns(line).runs, shown.from, shown.to) : []);
    const surfaces = $derived([...new Set(runs.map(run => run.surface))]);
    const tooLong = $derived(span * totalM / 1000 > PLACES_KM);

    // The start, the end, the nights, then the distances: a tick that would touch a kept one is left out.
    const ticks = $derived.by(() => {
        const nightName = (share: number) => pins.find(pin => !pin.number && Math.abs(pin.share - share) < 1e-6)?.label || 'Night';
        const length = distanceIn(span * totalM), first = distanceIn(shown.from * totalM);
        const step = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000].find(size => length / size <= Math.max(1, width / 80)) ?? 2000;
        const numbers = [];
        for (let value = Math.ceil(first / step) * step; value < first + length; value += step) numbers.push({ at: (value - first) / length, label: `${settings.units} ${value}` });
        const wanted = [
            { at: 0, label: shown.from ? nightName(shown.from) : orderedRoutePoints(trip)[0]?.label ?? '' },
            { at: 1, label: formatDistance(shown.to * totalM) },
            ...nights.filter(share => share > shown.from && share < shown.to).map(share => ({ at: x(share), label: nightName(share) })),
            ...numbers,
        ];
        // A label sits as `Lanes` places it: shifted left by its share of the axis.
        const extent = ({ at, label }: { at: number; label: string }) => {
            const size = label.length * TICK_CHAR_PX;
            return [at * width - at * size, at * width + (1 - at) * size];
        };
        const kept: typeof wanted = [];
        for (const tick of wanted) {
            const [left, right] = extent(tick);
            if (kept.every(other => { const [a, b] = extent(other); return right + TICK_GAP_PX <= a || left >= b + TICK_GAP_PX; })) kept.push(tick);
        }
        return kept;
    });
    const labels = $derived(facts?.climbs.map(climb => ({ from: climb.from * 1000 / totalM, to: climb.to * 1000 / totalM, text: formatPercent(climb.gradient) })) ?? []);

    const chip = $derived.by(() => {
        if (mark === undefined) return '';
        const height = profileHeightAt(mark, samples);
        const grade = grades[Math.max(0, sampleIndex(samples, mark) - 1)] ?? null;
        return [formatDistance(mark * totalM), height === null ? '' : formatHeight(height), grade === null ? '' : formatPercent(grade)].filter(Boolean).join(' · ');
    });

    /** The places of the chosen category, joined where their icons would overlap. */
    const groups = $derived.by(() => {
        // On the shown slice, so that a place on a loop does not land on another day.
        const found = (places.found ?? []).map(place => ({ place, at: nearestProgress(slice, place.coordinate) })).sort((a, b) => a.at - b.at);
        const joined: { at: number; names: string[] }[] = [];
        for (const { place, at } of found) {
            const last = joined.at(-1);
            if (last && (at - last.at) * width < JOIN_PX) last.names.push(place.label);
            else joined.push({ at, names: [place.label] });
        }
        return joined;
    });

    $effect(() => {
        if (!placeLane) return;
        const observer = new IntersectionObserver(entries => { if (entries.some(entry => entry.isIntersecting)) seen = true; });
        observer.observe(placeLane);
        return () => observer.disconnect();
    });

    $effect(() => {
        places = {};
        const wanted = category;
        if (!seen || !wanted || tooLong) return;
        let live = true;
        corridorPlaces(config.places, slice, PLACE_M / 1000).then(
            found => { if (live) places = { found: found.filter(place => place.category === wanted) }; },
            () => { if (live) places = { failed: true }; });
        return () => { live = false; };
    });

    // The details of a waypoint made from a mapped place load when its panel opens.
    $effect(() => {
        const point = pins.find(pin => pin.number === open)?.point, id = point?.placeId;
        if (!point || !id || !searchable.test(id) || untrack(() => id in details)) return;
        const abort = new AbortController();
        placeDetails(id, point.coordinate, abort.signal).then(found => { details = { ...details, [id]: found ?? null }; }, () => {});
        return () => abort.abort();
    });

    /** Opens the panel of a pin, moved left where it would leave the window. */
    function show(pin: HTMLElement, number: number | undefined) {
        const box = pin.getBoundingClientRect(), panel = Math.min(PANEL_PX, innerWidth * .9);
        shift = Math.max(8 - box.left, Math.min(-8, innerWidth - 8 - panel - box.left));
        open = number;
    }

    const surfaceName = (surface: Surface) => surface === 'Unknown' ? 'unknown' : surface.toLowerCase();
</script>

<!-- iOS Safari does not focus a tapped button, so a tap outside closes the panel here, not by `focusout`. -->
<svelte:document onpointerdown={event => { if (open !== undefined && !(event.target as Element).closest('.pin')) open = undefined; }} />

{#snippet about(point: RoutePoint)}
    {@const found = point.placeId ? details[point.placeId] : undefined}
    {@const website = websiteLink(found?.website)}
    <div class="panel" style:left={`${shift}px`}>
        <strong>{point.label}</strong>
        {#if point.placeKind}<small>{kindLabel(point.placeKind)}</small>{/if}
        {#if point.note}<p>{point.note}</p>{/if}
        {#if found?.opening_hours}<OpeningHours value={found.opening_hours} />{/if}
        {#if website || found?.phone}
            <p class="contacts">
                {#if website}<a href={website} target="_blank" rel="noopener noreferrer">Website</a>{/if}
                {#each phoneNumbers(found?.phone) as phone (phone)}
                    {@const link = phoneLink(phone)}
                    {#if link}<a href={link}>{phone}</a>{:else}<span>{phone}</span>{/if}
                {/each}
            </p>
        {/if}
    </div>
{/snippet}

{#snippet waypoints()}
    {#each shownPins as pin, i (i)}
        {#if pin.number}
            <!-- A link in the panel takes the focus: the panel closes only when the focus leaves the pin and its panel. -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <span class="pin" style:left={`${x(pin.share) * 100}%`}
                onpointerenter={event => { if (event.pointerType === 'mouse') show(event.currentTarget, pin.number); }}
                onpointerleave={event => { if (event.pointerType === 'mouse' && !event.currentTarget.contains(document.activeElement)) open = undefined; }}
                onfocusout={event => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) open = undefined; }}
                onkeydown={event => { if (event.key === 'Escape') { event.currentTarget.querySelector('button')!.focus(); open = undefined; } }}>
                <button type="button" aria-label={pin.label} aria-expanded={open === pin.number} onclick={event => show(event.currentTarget.parentElement!, pin.number)}>{pin.number}</button>
                {#if open === pin.number && pin.point}{@render about(pin.point)}{/if}
            </span>
        {:else}
            <span class="night" style:left={`${x(pin.share) * 100}%`}>Night</span>
        {/if}
    {/each}
{/snippet}

{#snippet elevation()}
    <ElevationRow line={profile} from={shown.from} to={shown.to} {labels} lines={nights} />
{/snippet}

{#snippet surface()}
    <div class="bar">
        {#each runs as run, i (i)}<span style:left={`${x(run.from) * 100}%`} style:width={`${(run.to - run.from) / span * 100}%`} style:background={surfaceBarColors[run.surface]}></span>{/each}
    </div>
    <span class="legend">
        {#each surfaces as surface (surface)}<span><i style:background={surfaceBarColors[surface]}></i>{surfaceName(surface)}</span>{/each}
    </span>
{/snippet}

{#snippet placeLabel()}
    <select aria-label="Places" bind:value={category}>
        <option value="">Places</option>
        {#each categoryIds as id (id)}<option value={id}>{placeCategories[id].label}</option>{/each}
    </select>
{/snippet}

{#snippet placeRow()}
    <div class="places" bind:this={placeLane} bind:clientWidth={width}>
        {#if category && tooLong}
            <span class="note">{days.length > 1 && day === 'all' ? 'Choose one day to see places' : 'Too long to show places'}</span>
        {:else if places.failed}
            <span class="note">Places are unavailable.</span>
        {:else if places.found && !groups.length}
            <span class="note">None within {formatDistance(PLACE_M)}</span>
        {:else if category}
            {#each groups as group, i (i)}
                <span class="place" style:left={`${group.at * 100}%`} title={group.names.join('\n')}>
                    <svg viewBox="0 0 24 24" aria-hidden="true"><path d={placeCategories[category].icon} /></svg>
                    {#if group.names.length > 1}<b>{group.names.length}</b>{/if}
                </span>
            {/each}
        {/if}
    </div>
{/snippet}

{#snippet pointer()}{chip}{/snippet}

<div class="rows">
    <Card>
        <header>
            <h2>From start to end</h2>
            {#if days.length > 1}
                <Segmented compact label="Shown days" value={day} onChange={value => day = value}
                    options={[{ value: 'all', label: days.length === 2 ? 'Both days' : 'All days' }, ...days.map(item => ({ value: String(item.number), label: `Day ${item.number}` }))]} />
            {/if}
        </header>
        <Lanes {ticks} pointer={mark !== undefined && inside(mark) ? x(mark) : undefined} chip={pointer}
            onpointer={at => onpoint(at === null ? null : shown.from + at * span)}
            rows={[
                { label: 'Waypoints', draw: waypoints, height: 24 },
                { label: 'Elevation', draw: elevation, height: 96 },
                ...runs.length ? [{ label: 'Surface', draw: surface }] : [],
                { label: placeLabel, draw: placeRow, height: 24 },
            ]} />
    </Card>
</div>

<style>
    header {
        display: flex;
        align-items: center;
        gap: 16px;
        margin-bottom: 14px;
    }
    h2 {
        margin: 0;
        font: 700 15px/1.3 var(--sans);
    }
    .pin, .night, .place {
        position: absolute;
        top: 50%;
        transform: translate(-50%, -50%);
    }
    .pin button {
        display: grid;
        place-items: center;
        width: 22px;
        height: 22px;
        padding: 0;
        border: 2px solid var(--panel);
        border-radius: 50%;
        background: var(--rust);
        color: var(--cream);
        font: 700 11px/1 var(--sans);
    }
    /* A pin keeps its disc in sight over a night label. */
    .pin {
        z-index: 1;
    }
    .pin:hover, .pin:focus-within {
        z-index: 5;
    }
    .night {
        padding: 1px 6px;
        border-radius: 4px;
        background: var(--parchment-2);
        color: var(--ink-soft);
        font-size: 12px;
    }
    .panel {
        position: absolute;
        top: calc(100% + 6px);
        display: flex;
        flex-direction: column;
        gap: 4px;
        width: min(260px, 90vw);
        padding: 12px 14px;
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--panel);
        box-shadow: 0 8px 24px rgb(28 27 20 / 0.18);
        font-size: 13px;
    }
    /* Covers the gap above the panel, so that the pointer does not leave the pin on its way to a link. */
    .panel::before {
        position: absolute;
        right: 0;
        bottom: 100%;
        left: 0;
        height: 7px;
        content: '';
    }
    .panel small {
        color: var(--ink-soft);
    }
    .panel p {
        margin: 2px 0 0;
    }
    .panel :global(.hours) {
        margin: 6px 0 0;
        padding: 8px 0 0;
        border-bottom: 0;
    }
    .contacts {
        display: flex;
        flex-wrap: wrap;
        gap: 4px 12px;
    }
    .contacts a {
        color: var(--forest);
    }
    .legend {
        display: flex;
        flex-wrap: wrap;
        gap: 2px 12px;
        margin-top: 5px;
        color: var(--ink-soft);
        font-size: 11.5px;
    }
    .legend span {
        display: flex;
        align-items: center;
        gap: 4px;
    }
    .legend i {
        width: 8px;
        height: 8px;
        border-radius: 2px;
    }
    .bar {
        position: relative;
        height: 10px;
        overflow: hidden;
        border-radius: 5px;
        background: var(--parchment-2);
    }
    .bar span {
        position: absolute;
        top: 0;
        bottom: 0;
    }
    select {
        width: 100%;
        padding: 5px 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
    }
    .places {
        position: relative;
        height: 100%;
    }
    .place svg {
        display: block;
        width: 18px;
        height: 18px;
        fill: none;
        stroke: var(--forest);
        stroke-width: 2;
        stroke-linecap: round;
        stroke-linejoin: round;
    }
    .place b {
        position: absolute;
        top: -6px;
        right: -10px;
        padding: 0 4px;
        border-radius: 7px;
        background: var(--forest);
        color: var(--panel);
        font: 700 10px/14px var(--sans);
    }
    .note {
        color: var(--ink-soft);
        font-size: 12.5px;
        line-height: 24px;
    }
</style>
