<script lang="ts">
    import { untrack } from 'svelte';
    import Card from '../../lib/ui/Card.svelte';
    import Segmented from '../../lib/ui/Segmented.svelte';
    import OpeningHours from '../planner/OpeningHours.svelte';
    import SurfaceLegend from '../planner/SurfaceLegend.svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import ElevationRow from './ElevationRow.svelte';
    import Menu from '../../lib/ui/Menu.svelte';
    import { placeAside } from './PlacePopup.svelte';
    import PlaceDisc from '../planner/PlaceDisc.svelte';
    import Lanes from '../../lib/ui/Lanes.svelte';
    import { distanceTicks, fitTicks } from './ticks';
    import type { RouteSummary } from '../../lib/library/records';
    import { lineCoordinates } from '../../lib/library/summary';
    import { phoneLink, phoneNumbers, websiteLink } from '../../lib/planner/contact-links';
    import { orderedRoutePoints, planView, type Place, type RoutePoint, type Trip } from '../../lib/planner/editor';
    import { cumulative, nearestProgress, routeSlice } from '../../lib/planner/geo';
    import { profileGrades } from '../../lib/planner/grade-data';
    import { config } from '../../lib/planner/map-data';
    import { corridorPlaces, type RoutePlace } from '../../lib/planner/place-index';
    import { categoryIds, placeCategories, type PlaceCategory } from '../../lib/planner/poi-kinds';
    import { formatPart, profileAscent, profileHeightAt, profileSamples, sampleIndex } from '../../lib/planner/profile-data';
    import type { Pin, RouteFacts } from '../../lib/planner/route-facts';
    import type { RoutingLine, Surface } from '../../lib/planner/routing';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import { placeDetails, type SearchPlace } from '../../lib/planner/search/types';
    import { surfaceBarColors, surfaceRuns, surfaceShares, surfaceWindow } from '../../lib/planner/surface-data';
    import { formatDistance, formatHeight, formatPercent } from '../../lib/settings/format';

    /** The bottom card of the Route page: the waypoints, the heights, the surface and the places along the route or one
     * day of it, on one axis. Shares are 0 to 1 of the route. Without the calculated line, the rows that the summary
     * holds show. A drag selects a part; with no part, a click on a climb in the elevation row selects the climb. */
    let { summary, trip, line, failed = false, facts, pins, mark, onpoint, part, onpart, onhover, onday,
        routePlaces = $bindable([]), litPlaces = $bindable([]), openPlaceId = $bindable(null) }: {
        summary: RouteSummary;
        trip: Trip;
        line?: RoutingLine;
        /** The line will not come, such as after a routing error: the surface lane no longer waits. */
        failed?: boolean;
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
        /** The places of the chosen category, which only the rows write. */
        routePlaces?: RoutePlace[];
        /** The ids of the places that the pointer lights, here or on the map, and of the place whose popup is open. */
        litPlaces?: string[];
        openPlaceId?: string | null;
    } = $props();

    /** Places load for at most PLACES_KM of the shown stretch. A place icon is ICON_PX wide and a count after it about
     * COUNT_CHAR_PX for each figure; icons that would touch join into one, whose list shows at most LIST_NAMES names. */
    const PLACES_KM = 300, ICON_PX = 24, COUNT_CHAR_PX = 7, LIST_NAMES = 30, PANEL_PX = 260;
    /** The elevation row is ELEVATION_PX high in a window at least WIDE_PX wide and TALL_PX high. The disc of a night keeps
     * NIGHT_PX from the middle of a waypoint pin. */
    const ELEVATION_PX = 120, WIDE_PX = 1200, TALL_PX = 800, ELEVATION_ROW = 1, NIGHT_PX = 20;
    const searchable = /^(?:[nwr]|Q)[1-9]\d*$/;

    /** The pointer on the axis of the shown stretch while it is over the elevation row or moved by the keys. */
    let onProfile = $state<number>(), innerWidth = $state(1440), innerHeight = $state(900);
    let day = $state('all'), category = $state<PlaceCategory | ''>(''), open = $state<number>(), shift = $state(0);
    /** The group of places whose list is open. */
    let listed = $state<number>();
    let placeLane = $state<HTMLElement>(), width = $state(0), seen = $state(false);
    /** The surface of the legend entry under the pointer or with the focus: the bar lights its stretches. */
    let litSurface = $state<Surface>();
    let placesState = $state<'' | 'loaded' | 'failed'>('');
    let details = $state.raw<Record<string, SearchPlace | null>>({});

    const coordinates = $derived(lineCoordinates(summary));
    const profile = $derived(line ?? { coordinates, elevation: summary.heights });
    const samples = $derived(profileSamples(profile));
    const grades = $derived(profileGrades(samples, cumulative(profile.coordinates).at(-1) ?? 0));
    const totalM = $derived(line ? planView(trip, line).total * 1000 : summary.lengthM);
    const days = $derived(facts?.days ?? []);
    const shown = $derived(days.find(item => String(item.number) === day) ?? { from: 0, to: 1 });
    const span = $derived(Math.max(1e-6, shown.to - shown.from));
    const x = (share: number) => (share - shown.from) / span;
    const inside = (share: number) => share >= shown.from && share <= shown.to;
    // The start tick names the night at the start of the shown stretch.
    const shownPins = $derived(pins.filter(pin => pin.number ? inside(pin.share) : pin.share > shown.from && pin.share <= shown.to));
    // The shares of a day belong to the calculated line.
    const slice = $derived(routeSlice(profile.coordinates, shown.from, shown.to));
    const nights = $derived(pins.filter(pin => !pin.number).map(pin => pin.share));
    const runs = $derived(line ? surfaceWindow(surfaceRuns(line), shown.from, shown.to) : []);
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
    const shownPart = $derived.by((): [number, number] | null => {
        if (!part || part[1] <= shown.from || part[0] >= shown.to) return null;
        return [Math.max(0, x(part[0])), Math.min(1, x(part[1]))];
    });
    // While the rows show a selected part, a click only clears it: no climb is under the pointer, to select or to light.
    // A part outside the shown day does not count, because the rows do not show it.
    const climbAt = (share: number) => shownPart ? -1 : climbs.findIndex(climb => share >= climb.from && share <= climb.to);
    const same = (a: [number, number] | null, b: { from: number; to: number }) => !!a && Math.abs(a[0] - b.from) < 1e-9 && Math.abs(a[1] - b.to) < 1e-9;
    const chosenClimb = $derived(climbs.findIndex(climb => same(part, climb)));
    const hoveredClimb = $derived(onProfile === undefined ? -1 : climbAt(shown.from + onProfile * span));
    const lit = $derived(hoveredClimb >= 0 ? hoveredClimb : chosenClimb >= 0 ? chosenClimb : undefined);
    // A climb gives its own length and gain, as the left card does; a profile with no heights gives no gain.
    const partText = $derived.by(() => {
        if (!part) return '';
        const climb = chosenClimb >= 0 ? facts?.climbs[chosenClimb] : undefined;
        const gain = climb ? climb.ascent : samples.some(sample => sample.height !== null) ? profileAscent(part[0], part[1], profile) : null;
        return formatPart(part[0] * totalM, part[1] * totalM, gain, climb ? (climb.to - climb.from) * 1000 : undefined);
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
        const found = routePlaces.map(place => ({ place, at: x(place.alongM / totalM) })).sort((a, b) => a.at - b.at);
        const joined: { at: number; places: RoutePlace[] }[] = [];
        const drawn = (count: number) => ICON_PX + (count > 1 ? 2 + String(count).length * COUNT_CHAR_PX : 0);
        for (const { place, at } of found) {
            const last = joined.at(-1);
            if (last && (at - last.at) * width < drawn(last.places.length + 1) + 2) last.places.push(place);
            else joined.push({ at, places: [place] });
        }
        return joined;
    });

    $effect(() => { onhover(hoveredClimb >= 0 ? [climbs[hoveredClimb].from, climbs[hoveredClimb].to] : null); });
    $effect(() => { onday(day === 'all' ? null : [shown.from, shown.to]); });
    // `listed` points into `groups`: new groups, after a resize or another day, close it.
    $effect(() => { void groups; listed = undefined; });

    $effect(() => {
        if (!placeLane) return;
        const observer = new IntersectionObserver(entries => { if (entries.some(entry => entry.isIntersecting)) seen = true; });
        observer.observe(placeLane);
        return () => observer.disconnect();
    });

    $effect(() => {
        [routePlaces, litPlaces, openPlaceId, placesState] = [[], [], null, ''];
        const wanted = category, along = slice, from = shown.from;
        if (!seen || !wanted || tooLong) return;
        let live = true;
        corridorPlaces(config.places, along, wanted).then(found => {
            if (!live) return;
            // On the shown slice, so that a place on a loop does not land on another day.
            routePlaces = found.map(item => ({ ...item, alongM: (from + nearestProgress(along, item.place.coordinate) * span) * totalM }));
            placesState = 'loaded';
        }, () => { if (live) placesState = 'failed'; });
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

    /** With no part in the rows, a click on a climb in the elevation row, or Enter, selects the climb. Any other click,
     * and each click while the rows show a part, clears the part. */
    function click(at: number, row?: number | null) {
        const index = row === ELEVATION_ROW || row === null ? climbAt(shown.from + at * span) : -1;
        onpart(index >= 0 ? [climbs[index].from, climbs[index].to] : null);
    }

    function point(at: number | null, row?: number | null) {
        onProfile = at !== null && (row === ELEVATION_ROW || row === null) ? at : undefined;
        onpoint(at === null ? null : shown.from + at * span);
    }

    /** One Escape closes one thing: an open dialog or menu first, and a field, a pin of the map or the popup that has the
     * focus, which close themselves; then a panel or the list, then the popup of a place, then the part, which `Lanes`
     * clears. The focus goes back to the place. */
    function closeOnEscape(event: KeyboardEvent) {
        if (event.key !== 'Escape' || event.defaultPrevented || document.querySelector('dialog:modal, menu')) return;
        if ((event.target as Element).closest?.('input, .route-pin, .map-callout') || (open === undefined && listed === undefined && !openPlaceId)) return;
        event.preventDefault();
        if (listed !== undefined) focusGroup();
        if (open === undefined && listed === undefined) openPlaceId = null;
        [open, listed] = [undefined, undefined];
    }

    const focusGroup = () => placeLane?.querySelector<HTMLElement>('.place.open > button')?.focus();

    /** The popup takes the group button as its opener: the name in the list goes with the list. That name takes no
     * `pointerleave`, so the lit places clear here. */
    function openPlace(id: string) {
        if (listed !== undefined) focusGroup();
        [openPlaceId, listed, litPlaces] = [id, undefined, []];
    }
</script>

<svelte:window bind:innerWidth bind:innerHeight onkeydowncapture={closeOnEscape} />
<!-- iOS Safari does not focus a tapped button, so a tap outside closes the panel here, not by `focusout`. -->
<svelte:document onpointerdown={event => {
    if (open !== undefined && !(event.target as Element).closest('.pin')) open = undefined;
    if (listed !== undefined && !(event.target as Element).closest('.place')) listed = undefined;
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
        {#each runs as run, i (i)}<span class:dim={litSurface && run.surface !== litSurface} style:left={`${x(run.from) * 100}%`} style:width={`${(run.to - run.from) / span * 100}%`} style:background={surfaceBarColors[run.surface]}></span>{/each}
    </div>
    {#if line}<SurfaceLegend shares={surfaceShares(runs)} bind:lit={litSurface} />{:else}<div class="legend-space"></div>{/if}
{/snippet}

{#snippet placeRow()}
    <div class="places" bind:this={placeLane}>
        {#if category && tooLong}
            <span class="note">{days.length > 1 && day === 'all' ? 'Choose one day to see places' : 'Too long to show places'}</span>
        {:else if placesState === 'failed'}
            <span class="note">Places are unavailable.</span>
        {:else if category && placesState === 'loaded' && !groups.length}
            <span class="note">None within {formatDistance(placeCategories[category].routeM)}</span>
        {:else if category}
            {@const icon = placeCategories[category].icon}
            {#each groups as group, i (i)}
                {@const ids = group.places.map(({ place }) => place.id)}
                {@const several = ids.length > 1}
                <span class="place" class:open={listed === i} style:left={`${group.at * 100}%`}
                    style:transform={group.at * width > width - 40 ? 'translate(-100%, -50%)' : undefined}>
                    <button type="button" aria-label={group.places.map(({ place }) => place.label).join(', ')} aria-expanded={several ? listed === i : undefined}
                        onpointerenter={() => litPlaces = ids} onpointerleave={() => litPlaces = []}
                        onclick={event => {
                            if (!several) return openPlace(ids[0]);
                            show(event.currentTarget.parentElement!, undefined);
                            listed = listed === i ? undefined : i;
                        }}>
                        {#if ids.some(id => id === openPlaceId || litPlaces.includes(id))}<PlaceDisc {icon} grow />
                        {:else}<svg viewBox="0 0 24 24" aria-hidden="true"><path d={icon} /></svg>{/if}
                        {#if several}<span class="count">{ids.length}</span>{/if}
                    </button>
                    {#if listed === i}
                        <div class="panel list" data-panel style:left={`${shift}px`}>
                            {#each group.places.slice(0, LIST_NAMES) as item (item.place.id)}
                                <button type="button" onpointerenter={() => litPlaces = [item.place.id]} onpointerleave={() => litPlaces = []}
                                    onclick={() => openPlace(item.place.id)}><span>{item.place.label}</span><small>{placeAside(item)}</small></button>
                            {/each}
                            {#if group.places.length > LIST_NAMES}<p class="more">and {group.places.length - LIST_NAMES} more</p>{/if}
                        </div>
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
        ...categoryIds.map(id => ({ label: placeCategories[id].label, checked: category === id, run: () => { category = id; listed = undefined; } })),
        ...category ? [{ label: 'Hide places', rule: true, run: () => { category = ''; listed = undefined; } }] : [],
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
                // The lane waits for the calculated line, so the answer moves nothing.
                ...runs.length || (!line && !failed) ? [{ label: 'Surface', draw: surface }] : [],
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
    /* The bar stands beside its label; its legend follows under it. */
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
    /* One line of the legend. */
    .legend-space {
        height: 16px;
    }
    .bar .dim {
        opacity: .2;
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
    .place :global(.place-disc) {
        margin: 0 1px;
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
    /* The name over where the place is, so that the list keeps the width of a panel. */
    .list button {
        display: flex;
        flex-direction: column;
        gap: 1px;
        padding: 6px 8px;
        border: 0;
        border-radius: 5px;
        background: none;
        color: var(--ink);
        text-align: left;
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
