<script lang="ts">
    import { untrack } from 'svelte';
    import Card from '../../lib/ui/Card.svelte';
    import Segmented from '../../lib/ui/Segmented.svelte';
    import OpeningHours from '../planner/OpeningHours.svelte';
    import Icon from '../planner/PlannerIcon.svelte';
    import ElevationRow from './ElevationRow.svelte';
    import Menu from './Menu.svelte';
    import Lanes from './Lanes.svelte';
    import { distanceTicks, fitTicks } from './ticks';
    import type { RouteSummary } from '../../lib/library/records';
    import { lineCoordinates } from '../../lib/library/summary';
    import { phoneLink, phoneNumbers, websiteLink } from '../../lib/planner/contact-links';
    import { orderedRoutePoints, planView, type Place, type RoutePoint, type Trip } from '../../lib/planner/editor';
    import { cumulative, nearestProgress, routeSlice, type Coordinate } from '../../lib/planner/geo';
    import { profileGrades } from '../../lib/planner/grade-data';
    import { config } from '../../lib/planner/map-data';
    import { corridorPlaces } from '../../lib/planner/place-index';
    import { categoryIds, placeCategories, type PlaceCategory } from '../../lib/planner/poi-kinds';
    import { profileAscent, profileHeightAt, profileSamples, sampleIndex } from '../../lib/planner/profile-data';
    import type { Pin, RouteFacts } from '../../lib/planner/route-facts';
    import type { RoutingLine } from '../../lib/planner/routing';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import { placeDetails, type SearchPlace } from '../../lib/planner/search/types';
    import { surfaceBarColors, surfaceRuns, surfaceWindow } from '../../lib/planner/surface-data';
    import { formatAlong, formatDistance, formatHeight, formatPercent } from '../../lib/settings/format';

    /** The bottom card of the Route page: the waypoints, the heights, the surface and the places along the route or one
     * day of it, on one axis. Shares are 0 to 1 of the route. Without the calculated line, the rows that the summary
     * holds show. A drag selects a part; a click on a climb in the elevation row selects the climb. */
    let { summary, trip, line, facts, pins, mark, onpoint, part, onpart, onhover, onday, onplace }: {
        summary: RouteSummary;
        trip: Trip;
        line?: RoutingLine;
        facts?: RouteFacts;
        pins: Pin[];
        mark?: number;
        onpoint: (share: number | null) => void;
        /** The selected part, as shares of the whole route. */
        part: [number, number] | null;
        onpart: (part: [number, number] | null) => void;
        /** The stretch of the climb under the pointer. */
        onhover: (stretch: [number, number] | null) => void;
        /** The stretch of the chosen day, or null for the whole route. */
        onday: (stretch: [number, number] | null) => void;
        /** The place whose panel is open. */
        onplace: (at: Coordinate | null) => void;
    } = $props();

    /** Places load within PLACE_M of the shown stretch, for at most PLACES_KM of it. A place icon is ICON_PX wide and a
     * count after it about COUNT_CHAR_PX for each figure; icons that would touch join into one, whose list shows at most
     * LIST_NAMES names. */
    const PLACE_M = 1000, PLACES_KM = 300, ICON_PX = 24, COUNT_CHAR_PX = 7, LIST_NAMES = 30, PANEL_PX = 260;
    /** The elevation row is ELEVATION_PX high in a window at least WIDE_PX wide and TALL_PX high. The disc of a night keeps
     * NIGHT_PX from the middle of a waypoint pin. */
    const ELEVATION_PX = 120, WIDE_PX = 1200, TALL_PX = 800, ELEVATION_ROW = 1, NIGHT_PX = 20;
    const searchable = /^(?:[nwr]|Q)[1-9]\d*$/;

    /** The pointer on the axis of the shown stretch while it is over the elevation row or moved by the keys. */
    let onProfile = $state<number>(), innerWidth = $state(1440), innerHeight = $state(900);
    let day = $state('all'), category = $state<PlaceCategory | ''>(''), open = $state<number>(), shift = $state(0);
    /** The open group of places, and the place of a group of several that the rider chose. */
    let openPlace = $state<{ group: number; place?: number }>();
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

    // The start, the end, the nights, then the distances.
    const ticks = $derived.by(() => {
        const nightName = (share: number) => pins.find(pin => !pin.number && Math.abs(pin.share - share) < 1e-6)?.label || 'Night';
        return fitTicks([
            { at: 0, label: shown.from ? nightName(shown.from) : orderedRoutePoints(trip)[0]?.label ?? '' },
            { at: 1, label: formatDistance(shown.to * totalM) },
            ...nights.filter(share => share > shown.from && share < shown.to).map(share => ({ at: x(share), label: nightName(share) })),
            ...distanceTicks(shown.from * totalM, shown.to * totalM, width),
        ], width);
    });

    const climbs = $derived(facts?.climbs.map(climb => ({
        from: climb.from * 1000 / totalM, to: climb.to * 1000 / totalM,
        text: `${formatDistance((climb.to - climb.from) * 1000)} · ${formatPercent(climb.gradient)}`, short: formatPercent(climb.gradient),
    })) ?? []);
    const climbAt = (share: number) => climbs.findIndex(climb => share >= climb.from && share <= climb.to);
    const same = (a: [number, number] | null, b: { from: number; to: number }) => !!a && Math.abs(a[0] - b.from) < 1e-9 && Math.abs(a[1] - b.to) < 1e-9;
    const chosenClimb = $derived(climbs.findIndex(climb => same(part, climb)));
    const hoveredClimb = $derived(onProfile === undefined ? -1 : climbAt(shown.from + onProfile * span));
    const lit = $derived(hoveredClimb >= 0 ? hoveredClimb : chosenClimb >= 0 ? chosenClimb : undefined);
    const shownPart = $derived.by((): [number, number] | null => {
        if (!part || part[1] <= shown.from || part[0] >= shown.to) return null;
        return [Math.max(0, x(part[0])), Math.min(1, x(part[1]))];
    });
    // A climb gives its own length and gain, as the left card does; a profile with no heights gives no gain.
    const partText = $derived.by(() => {
        if (!part) return '';
        const climb = chosenClimb >= 0 ? facts?.climbs[chosenClimb] : undefined;
        const gain = climb ? climb.ascent : samples.some(sample => sample.height !== null) ? profileAscent(part[0], part[1], profile) : null;
        return [`${formatAlong(part[0] * totalM)} to ${formatAlong(part[1] * totalM)}`, formatDistance(climb ? (climb.to - climb.from) * 1000 : (part[1] - part[0]) * totalM),
            gain === null ? '' : `${formatHeight(gain)} up`].filter(Boolean).join(' · ');
    });
    const elevationHeight = $derived(innerWidth >= WIDE_PX && innerHeight >= TALL_PX ? ELEVATION_PX : 96);

    const chip = $derived.by(() => {
        if (mark === undefined) return '';
        const height = profileHeightAt(mark, samples);
        const grade = grades[Math.max(0, sampleIndex(samples, mark) - 1)] ?? null;
        return [formatDistance(mark * totalM), height === null ? '' : formatHeight(height), grade === null ? '' : formatPercent(grade)].filter(Boolean).join(' · ');
    });

    /** The places of the chosen category, joined where their icons with the count would touch. */
    const groups = $derived.by(() => {
        // On the shown slice, so that a place on a loop does not land on another day.
        const found = (places.found ?? []).map(place => ({ place, at: nearestProgress(slice, place.coordinate) })).sort((a, b) => a.at - b.at);
        const joined: { at: number; places: Place[] }[] = [];
        const drawn = (count: number) => ICON_PX + (count > 1 ? 2 + String(count).length * COUNT_CHAR_PX : 0);
        for (const { place, at } of found) {
            const last = joined.at(-1);
            if (last && (at - last.at) * width < drawn(last.places.length + 1) + 2) last.places.push(place);
            else joined.push({ at, places: [place] });
        }
        return joined;
    });
    const chosenPlace = $derived.by(() => {
        const group = openPlace && groups[openPlace.group];
        if (!group) return undefined;
        return group.places.length === 1 ? group.places[0] : openPlace!.place === undefined ? undefined : group.places[openPlace!.place];
    });

    $effect(() => { onhover(hoveredClimb >= 0 ? [climbs[hoveredClimb].from, climbs[hoveredClimb].to] : null); });
    $effect(() => { onday(day === 'all' ? null : [shown.from, shown.to]); });
    $effect(() => { onplace(chosenPlace ? chosenPlace.coordinate : null); });
    // `openPlace` points into `groups`: new groups, after a resize or another day, close it.
    $effect(() => { void groups; openPlace = undefined; });

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

    // The details of a waypoint or a place made from a mapped place load when its panel opens.
    $effect(() => {
        const point = chosenPlace ?? pins.find(pin => pin.number === open)?.point, id = point?.placeId;
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

    /** A click on a climb in the elevation row, or Enter, selects the climb; another click clears the part. */
    function click(at: number, row?: number | null) {
        const index = row === ELEVATION_ROW || row === null ? climbAt(shown.from + at * span) : -1;
        onpart(index >= 0 ? [climbs[index].from, climbs[index].to] : null);
    }

    function point(at: number | null, row?: number | null) {
        onProfile = at !== null && (row === ELEVATION_ROW || row === null) ? at : undefined;
        onpoint(at === null ? null : shown.from + at * span);
    }

    /** Escape closes an open panel before the part, which `Lanes` clears; the focus goes back to the place. */
    function closeOnEscape(event: KeyboardEvent) {
        if (event.key !== 'Escape' || (open === undefined && !openPlace)) return;
        event.preventDefault();
        if (openPlace) placeLane?.querySelector<HTMLElement>('.place.open > button')?.focus();
        [open, openPlace] = [undefined, undefined];
    }

    const kmOf = (place: Place) => formatDistance((shown.from + nearestProgress(slice, place.coordinate) * span) * totalM);
</script>

<svelte:window bind:innerWidth bind:innerHeight onkeydowncapture={closeOnEscape} />
<!-- iOS Safari does not focus a tapped button, so a tap outside closes the panel here, not by `focusout`. -->
<svelte:document onpointerdown={event => {
    if (open !== undefined && !(event.target as Element).closest('.pin')) open = undefined;
    if (openPlace && !(event.target as Element).closest('.place')) openPlace = undefined;
}} />

{#snippet about(point: RoutePoint)}
    {@const found = point.placeId ? details[point.placeId] : undefined}
    {@const own = point as Partial<Place>}
    {@const website = websiteLink(found?.website ?? own.website)}
    {@const phone = found?.phone ?? own.phone}
    {@const hours = found?.opening_hours ?? own.openingHours}
    <div class="panel" data-panel style:left={`${shift}px`}>
        <strong>{point.label}</strong>
        {#if point.placeKind}<small>{kindLabel(point.placeKind)}</small>{/if}
        {#if point.note}<p>{point.note}</p>{/if}
        {#if hours}<OpeningHours value={hours} />{/if}
        {#if website || phone}
            <p class="contacts">
                {#if website}<a href={website} target="_blank" rel="noopener noreferrer">Website</a>{/if}
                {#each phoneNumbers(phone) as number (number)}
                    {@const link = phoneLink(number)}
                    {#if link}<a href={link}>{number}</a>{:else}<span>{number}</span>{/if}
                {/each}
            </p>
        {/if}
    </div>
{/snippet}

{#snippet waypoints()}
    <div class="measure" bind:clientWidth={width}></div>
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
            <!-- At an end of the axis the disc stays inside the card. Beside a waypoint it steps to the far side, so the
                 pin does not hide it. -->
            {@const gap = shownPins.map(other => other.number ? (x(pin.share) - x(other.share)) * width : Infinity).find(px => Math.abs(px) < NIGHT_PX)}
            <span class="night" role="img" aria-label="Night" style:left={`${x(pin.share) * 100}%`}
                style:margin-left={gap === undefined ? undefined : `${(gap < 0 ? -NIGHT_PX : NIGHT_PX) - gap}px`}
                style:transform={x(pin.share) > .97 ? 'translate(-100%, -50%)' : x(pin.share) < .03 ? 'translate(0, -50%)' : undefined}><Icon name="moon" size={11} /></span>
        {/if}
    {/each}
{/snippet}

{#snippet elevation()}
    <ElevationRow line={profile} from={shown.from} to={shown.to} {climbs} {lit} {part} lines={nights} />
{/snippet}

{#snippet surface()}
    <div class="bar">
        {#each runs as run, i (i)}<span style:left={`${x(run.from) * 100}%`} style:width={`${(run.to - run.from) / span * 100}%`} style:background={surfaceBarColors[run.surface]}></span>{/each}
    </div>
    <span class="legend">
        {#each surfaces as surface (surface)}<span><i style:background={surfaceBarColors[surface]}></i>{surface}</span>{/each}
    </span>
{/snippet}

{#snippet placeRow()}
    <div class="places" bind:this={placeLane}>
        {#if category && tooLong}
            <span class="note">{days.length > 1 && day === 'all' ? 'Choose one day to see places' : 'Too long to show places'}</span>
        {:else if places.failed}
            <span class="note">Places are unavailable.</span>
        {:else if places.found && !groups.length}
            <span class="note">None within {formatDistance(PLACE_M)}</span>
        {:else if category}
            {#each groups as group, i (i)}
                <span class="place" class:open={openPlace?.group === i} style:left={`${group.at * 100}%`}
                    style:transform={group.at * width > width - 40 ? 'translate(-100%, -50%)' : undefined}>
                    <button type="button" aria-label={group.places.map(place => place.label).join(', ')} aria-expanded={openPlace?.group === i}
                        onclick={event => { show(event.currentTarget.parentElement!, undefined); openPlace = openPlace?.group === i ? undefined : { group: i }; }}>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d={placeCategories[category].icon} /></svg>
                        {#if group.places.length > 1}<span class="count">{group.places.length}</span>{/if}
                    </button>
                    {#if openPlace?.group === i}
                        {#if chosenPlace}
                            {@render about(chosenPlace)}
                        {:else}
                            <div class="panel list" data-panel style:left={`${shift}px`}>
                                {#each group.places.slice(0, LIST_NAMES) as place, j (j)}
                                    <button type="button" onclick={() => openPlace = { group: i, place: j }}><span>{place.label}</span><small>{kmOf(place)}</small></button>
                                {/each}
                                {#if group.places.length > LIST_NAMES}<p class="more">and {group.places.length - LIST_NAMES} more</p>{/if}
                            </div>
                        {/if}
                    {/if}
                </span>
            {/each}
        {/if}
    </div>
{/snippet}

{#snippet pointer()}{chip}{/snippet}

{#snippet header()}
    {#if days.length > 1}
        <Segmented compact label="Shown days" value={day} onChange={value => { day = value; onpart(null); }}
            options={[{ value: 'all', label: days.length === 2 ? 'Both days' : 'All days' }, ...days.map(item => ({ value: String(item.number), label: `Day ${item.number}` }))]} />
    {/if}
    <Menu label="Places" left items={[
        ...categoryIds.map(id => ({ label: placeCategories[id].label, checked: category === id, run: () => { category = id; openPlace = undefined; } })),
        ...category ? [{ label: 'Hide places', rule: true, run: () => { category = ''; openPlace = undefined; } }] : [],
    ]} />
{/snippet}

<div class="rows">
    <Card>
        <Lanes label="Profile" {ticks} ticksAfter={ELEVATION_ROW + 1} {header} pointer={mark !== undefined && inside(mark) ? x(mark) : undefined}
            chip={pointer} onpointer={point} part={shownPart} {partText} onclick={click}
            onpart={next => onpart(next && [shown.from + next[0] * span, shown.from + next[1] * span])}
            rows={[
                { label: 'Waypoints', draw: waypoints, height: 24 },
                { label: 'Elevation', draw: elevation, height: elevationHeight },
                ...runs.length ? [{ label: 'Surface', draw: surface }] : [],
                ...category ? [{ label: placeCategories[category].label, draw: placeRow, height: 28 }] : [],
            ]} />
    </Card>
</div>

<style>
    .measure {
        position: absolute;
        inset: 0;
    }
    .pin, .night, .place {
        position: absolute;
        top: 50%;
        transform: translate(-50%, -50%);
    }
    .pin button, .night {
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
    /* A pin keeps its disc in sight over a night. */
    .pin {
        z-index: 1;
    }
    .pin:hover, .pin:focus-within {
        z-index: 5;
    }
    /* The night is a disc with the moon, as on the map. */
    .night {
        box-sizing: border-box;
        background: var(--ink);
        color: var(--panel);
    }
    .panel {
        position: absolute;
        z-index: 6;
        top: calc(100% + 6px);
        display: flex;
        flex-direction: column;
        gap: 4px;
        width: max-content;
        max-width: min(260px, 90vw);
        padding: 10px 12px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        background: var(--panel);
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
    /* The bar stands beside its label; its key follows under it. */
    .legend {
        display: flex;
        gap: 12px;
        overflow: hidden;
        height: 16px;
        color: var(--ink-soft);
        font-size: 11px;
        line-height: 16px;
        white-space: nowrap;
    }
    @media (max-width: 760px) {
        .legend {
            flex-wrap: wrap;
            gap: 0 10px;
            height: auto;
        }
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
        margin: 3px 0;
        overflow: hidden;
        border-radius: 5px;
        background: var(--parchment-2);
    }
    .bar span {
        position: absolute;
        top: 0;
        bottom: 0;
    }
    .places {
        position: relative;
        height: 100%;
    }
    /* The icon centres on the place; the count follows it. */
    .place {
        transform: translate(-12px, -50%);
    }
    .place.open {
        z-index: 5;
    }
    .place > button {
        display: flex;
        align-items: center;
        gap: 2px;
        height: 24px;
        padding: 0;
        border: 0;
        background: none;
        color: var(--ink-soft);
    }
    .place > button:hover, .place.open > button {
        color: var(--ink);
    }
    .place svg {
        display: block;
        width: 18px;
        height: 18px;
        margin: 0 3px;
        fill: none;
        stroke: currentColor;
        stroke-width: 2;
        stroke-linecap: round;
        stroke-linejoin: round;
    }
    .count {
        margin-left: -3px;
        font: 600 11px/1 var(--sans);
        font-variant-numeric: tabular-nums;
    }
    /* The places row is the last row of the card: its panel opens upwards. */
    .place .panel {
        top: auto;
        bottom: calc(100% + 6px);
        max-height: 240px;
        overflow: auto;
    }
    .list {
        gap: 0;
        min-width: 180px;
        padding: 4px;
    }
    .list button {
        display: flex;
        justify-content: space-between;
        gap: 16px;
        padding: 6px 8px;
        border: 0;
        border-radius: 5px;
        background: none;
        color: var(--ink);
        text-align: left;
        white-space: nowrap;
    }
    .list button:hover {
        background: var(--parchment-2);
    }
    .list .more {
        margin: 0;
        padding: 6px 8px;
        color: var(--ink-soft);
    }
    .note {
        color: var(--ink-soft);
        font-size: 12.5px;
        line-height: 28px;
    }
</style>
