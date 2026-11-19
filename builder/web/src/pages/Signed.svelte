<!-- The page of a signed route of the catalog: the cover, the map, the numbers, the profile with the surface and the places,
     the description, the official page and "Plan this route". It uses the parts of the Route page and reads no library. -->
<script lang="ts">
    import { untrack } from 'svelte';
    import Detail from '../components/route/Detail.svelte';
    import Facts from '../components/route/Facts.svelte';
    import Picture from '../components/route/Picture.svelte';
    import Rows from '../components/route/Rows.svelte';
    import { download, routeCourse } from '../components/route/download';
    import TrailMarker from '../components/planner/TrailMarker.svelte';
    import { initCore } from '../lib/core/bridge';
    import { writeFitCourse } from '../lib/core/fit';
    import { writeGpx } from '../lib/core/gpx';
    import { holdCover, type RouteCover } from '../lib/covers/route-pictures';
    import type { Library } from '../lib/library/store';
    import { lineCoordinates } from '../lib/library/summary';
    import { emptyTrip, planView } from '../lib/planner/editor';
    import { planSummary } from '../lib/planner/library';
    import { config, coversCell } from '../lib/planner/map-data';
    import type { RoutePlace } from '../lib/planner/place-index';
    import { catalog } from '../lib/planner/route-catalog';
    import { routeColor } from '../lib/planner/route-colors';
    import { routeFacts, routePins, type Climb } from '../lib/planner/route-facts';
    import { LegCache } from '../lib/planner/route-legs';
    import { routeWebsite } from '../lib/planner/route-overlays';
    import { calculateLine, type RoutingLine } from '../lib/planner/routing';
    import { planTrip, routeBase } from '../lib/planner/signed-route-plan';
    import { gradeName, routePlan, type CatalogRecord, type RouteRecord, type TrailPart } from '../lib/planner/signed-routes';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';
    import Menu from '../lib/ui/Menu.svelte';
    import { kindName } from './explore/SignedCard.svelte';
    import StageStrip from './explore/StageStrip.svelte';
    import { exploreAt, readAddress, shownDescription, signedHref, stagePart } from './explore/signed-page';
    import { signedItem } from './routes/filter';

    let {}: { library: Library; active: boolean } = $props();

    /** The page lays out its picture when the cover answers, or after this time with the map alone: from a card the cover
     * comes from the browser cache within it, so the cover and the map show together. */
    const COVER_WAIT_MS = 300;

    const address = $derived(readAddress(router.id ?? ''));
    /** The records of the cell of the address: undefined while they load, null for a cell outside the release. */
    let cell = $state.raw<{ key: string; records?: CatalogRecord[] | null; failed?: boolean }>();
    let attempt = $state(0);
    const recordOf = (id: number) => catalog.record(id);
    const record = $derived(address && cell?.key === address.cell
        ? cell.records?.find((one): one is RouteRecord => one.id === address.id && !!one.line_udeg && !!one.via) : undefined);
    const gone = $derived(!address || address.zoom !== 9 || cell?.records === null || (!!cell?.records && !record));

    const legs = new LegCache();
    let routed = $state.raw<{ record: CatalogRecord; line?: RoutingLine; error?: string }>();
    let cover = $state.raw<RouteCover>(), laid = $state(false);
    let trail = $state.raw<{ record: CatalogRecord; parts?: TrailPart[]; numbered: boolean }>();
    let share = $state<number>(), part = $state<[number, number] | null>(null), hover = $state<[number, number] | null>(null);
    let day = $state<[number, number] | null>(null);
    let routePlaces = $state.raw<RoutePlace[]>([]), litPlaces = $state.raw<string[]>([]), openPlaceId = $state<string | null>(null);

    const item = $derived(record && signedItem(record, exploreAt(record).chips.bike!));
    // The time is at the pace of a new plan.
    const trip = $derived(record && planTrip(routeBase(record, emptyTrip()), routePlan(record), record.loop));
    const line = $derived(routed?.record === record ? routed?.line : undefined);
    const error = $derived(routed?.record === record ? routed?.error : undefined);
    // The line, its heights, the time and the highest point come from the answer, so the map and the rows measure along one
    // line; the length and the climb stay those of the card. Before the answer, the record's line with no highest point.
    const summary = $derived(item && (line ? { ...planSummary(trip!, line), lengthM: item.summary.lengthM, climbM: item.summary.climbM }
        : { ...item.summary, highestM: null }));
    const facts = $derived(trip && line ? routeFacts(trip, line) : undefined);
    const pins = $derived(trip && summary ? routePins(trip, lineCoordinates(summary), line) : []);
    const focus = $derived(part ?? day);
    const climbShare = (climb: Climb): [number, number] => {
        const total = planView(trip!, line!).total;
        return [climb.from / total, climb.to / total];
    };
    const opened = $derived(part && line ? facts?.climbs.find(climb => climbShare(climb).every((end, i) => Math.abs(end - part![i]) < 1e-9)) : undefined);

    const stage = $derived(record ? stagePart(record, recordOf) : undefined);
    /** The trail of a stage or a section, whose strip the page shows and whose name opens it on Explore. */
    const longId = $derived(record && (record.trail ?? record.parent) !== undefined ? stage!.trailId : undefined);
    const colour = $derived(record ? routeColor(longId ?? record.id) : undefined);
    const trailName = $derived(stage?.part && (stage.long?.name ?? stage.long?.ref));
    const website = $derived(routeWebsite(record?.website));
    const description = $derived(record && shownDescription(record, recordOf));
    const lead = $derived(record && [record.loop ? 'Loop' : 'One way', kindName(record),
        ...record.kind !== 'bicycle' && record.hardest !== undefined ? [gradeName(record.kind, record.hardest)] : []].join(' · '));
    /** A section has its number in its id, −(relation × 100 + section); its part has the stage number of its cut stage. */
    const partName = (one: TrailPart) => one.id < 0 ? `Section ${-one.id % 100}` : one.stage !== undefined ? `Stage ${one.stage}` : undefined;
    /** The parts before and after this one in its trail, when the catalog holds them. */
    const neighbours = $derived.by(() => {
        const parts = trail?.parts, at = parts?.findIndex(one => one.id === record?.id) ?? -1;
        const link = (one: TrailPart | undefined, word: string) => one && !one.missing && one.cells && signedHref({ id: one.id, cells: one.cells }, config.bounds)
            ? { href: signedHref({ id: one.id, cells: one.cells }, config.bounds)!, label: partName(one) ?? word } : undefined;
        return at < 0 ? {} : { before: link(parts![at - 1], 'Part before'), after: link(parts![at + 1], 'Part after') };
    });

    $effect(() => {
        void attempt;
        const target = address;
        cell = undefined;
        if (!target || target.zoom !== 9) return;
        // A cell id that the grid does not have gives no file.
        if (!coversCell(config.bounds, target.cell)) return void (cell = { key: target.cell, records: null });
        let live = true;
        catalog.cell(target.cell).then(records => { if (live) cell = { key: target.cell, records }; },
            () => { if (live) cell = { key: target.cell, failed: true }; });
        return () => { live = false; };
    });

    // A new route starts with no selection and no place.
    $effect(() => {
        void record;
        untrack(() => {
            [share, part, hover, day] = [undefined, null, null, null];
            [routePlaces, litPlaces, openPlaceId] = [[], [], null];
        });
    });

    $effect(() => {
        const target = record, plan = trip;
        if (!target || !plan) return;
        const abort = new AbortController();
        calculateLine(plan, abort.signal, legs).then(
            found => { if (!abort.signal.aborted) routed = { record: target, line: found }; },
            reason => { if (!abort.signal.aborted) routed = { record: target, error: reason instanceof Error ? reason.message : 'Routing is unavailable.' }; });
        return () => abort.abort();
    });

    $effect(() => {
        const target = record;
        [cover, laid] = [undefined, false];
        if (!target) return;
        const { cover: loading, release } = holdCover(target);
        let live = true;
        const timer = setTimeout(() => { if (live) laid = true; }, COVER_WAIT_MS);
        void loading.then(found => {
            if (!live) return;
            [cover, laid] = [found, true];
        });
        return () => { live = false; clearTimeout(timer); release(); };
    });

    $effect(() => {
        const id = longId;
        trail = undefined;
        if (id === undefined) return;
        let live = true;
        catalog.trail(id).then(found => {
            if (live && found) trail = { record: found, parts: found.parts, numbered: (found.stage_count ?? 0) >= 2 };
        }, () => undefined);
        return () => { live = false; };
    });

    /** Back when Explore is the page before this one in this tab; else Explore opens on this route. */
    function toExplore(event: MouseEvent, show?: number) {
        event.preventDefault();
        if (router.from === 'explore' && show === undefined) history.back();
        else if (record) router.go(ROUTES.explore, { explore: exploreAt(record, show) });
    }

    function plan() {
        router.go(`${ROUTES.planner}/new`, { signed: { route: record, plan: routePlan(record!), back: exploreAt(record!) } });
    }

    const name = $derived(item?.name ?? '');
    const course = () => routeCourse(name, line!, facts!.stops);
    const exports = $derived([
        { label: 'GPX', disabled: !facts, run: () => void initCore().then(() => download(writeGpx(course()), 'application/gpx+xml', name, 'gpx')) },
        { label: 'FIT', disabled: !facts, run: () => void initCore().then(() => download(writeFitCourse({ ...course(), elapsedS: summary?.seconds ?? 0 }) as Uint8Array<ArrayBuffer>, 'application/vnd.ant.fit', name, 'fit')) },
    ]);
    const message = $derived(gone ? 'This route is no longer in the map data.' : cell?.failed ? 'The route could not load.' : !record ? 'Loading…' : undefined);
</script>

<div class="signed" style:--route={colour}>
    <Detail {message} {name} back={{ label: '‹ Explore', href: ROUTES.explore, onclick: record ? event => toExplore(event) : undefined }}>
        {#snippet action()}
            {#if gone && address}
                <a class="action" href={ROUTES.explore} onclick={event => { event.preventDefault(); router.go(ROUTES.explore, { explore: { chips: { bike: 'hiking' }, view: address.box } }); }}>Show this area on Explore</a>
            {:else if cell?.failed}
                <button type="button" class="action" onclick={() => attempt++}>Retry</button>
            {/if}
        {/snippet}
        {#snippet mark()}{#if record}<TrailMarker symbol={record.symbol} ref={record.ref} />{/if}{/snippet}
        {#snippet tags()}
            {#if stage?.part}
                <span class="plain">{stage.part}{#if trailName}{' · '}<a class="trail" href={ROUTES.explore} onclick={event => toExplore(event, longId)}>{trailName}</a>{/if}</span>
            {/if}
        {/snippet}
        {#snippet buttons()}
            <button type="button" class="button primary" onclick={plan}>Plan this route</button>
            {#if website}<a class="button" href={website} target="_blank" rel="noopener noreferrer">Official page ↗</a>{/if}
            <Menu label="Export" items={exports} />
        {/snippet}
        {#snippet left()}
            {#if trip && summary}
                <Facts {summary} {trip} {line} {facts} {error} changed={false} {opened} {lead} more={about} {foot}
                    onopen={climb => part = climb ? climbShare(climb) : null} />
            {/if}
        {/snippet}
        {#snippet picture()}
            {#if laid && summary && item && colour}
                <Picture {summary} {pins} signed={{ cover: cover && { ...cover, summary: item.summary }, colour, credits: record?.credit ? [record.credit] : [] }}
                    mark={share} onpoint={next => share = next ?? undefined} part={focus} lit={hover ?? focus} {routePlaces} bind:litPlaces bind:openPlaceId />
            {:else}
                <div class="waiting"></div>
            {/if}
        {/snippet}
        {#if trip && summary}
            {#key record}<Rows {summary} {trip} {line} failed={!!error} {facts} {pins} mark={share} onpoint={next => share = next ?? undefined}
                {part} onpart={next => part = next} onhover={next => hover = next} onday={next => day = next}
                bind:routePlaces bind:litPlaces bind:openPlaceId />{/key}
        {/if}
    </Detail>
</div>

{#snippet about()}
    {#if trail?.parts && record}
        <div class="strip">
            <StageStrip parts={trail.parts} numbered={trail.numbered} color={colour!} marked={record.id} />
            {#if neighbours.before || neighbours.after}
                <p class="steps">
                    {#if neighbours.before}<a href={neighbours.before.href}>‹ {neighbours.before.label}</a>{/if}
                    {#if neighbours.after}<a class="after" href={neighbours.after.href}>{neighbours.after.label} ›</a>{/if}
                </p>
            {/if}
        </div>
    {/if}
    {#if description}<p class="description">{description}</p>{/if}
{/snippet}

{#snippet foot()}
    <p class="credits">
        {#if record?.operator}<span>{record.operator}</span>{/if}
        {#if record?.credit}<span>Route data: {record.credit}</span>{/if}
        <span>Route from OpenStreetMap</span>
    </p>
{/snippet}

<style>
    .signed {
        height: 100%;
    }
    .signed :global(.title > :first-child:not(h1, input)) {
        flex: none;
    }
    .trail, .action {
        color: inherit;
    }
    button.action {
        padding: 0;
        border: 0;
        background: none;
        font: inherit;
        text-decoration: underline;
    }
    .strip {
        margin-top: 16px;
    }
    .steps {
        display: flex;
        margin: 6px 0 0;
        font-size: 13px;
    }
    .steps a {
        color: var(--ink-soft);
    }
    .steps .after {
        margin-left: auto;
    }
    .description {
        margin: 14px 0 0;
    }
    .credits {
        display: flex;
        flex-wrap: wrap;
        gap: 2px 12px;
        margin: 0;
    }
    .waiting {
        min-height: 0;
    }
    @media (max-width: 760px) {
        .signed {
            height: auto;
        }
        .waiting {
            order: -1;
            aspect-ratio: 3 / 2;
        }
    }
</style>
