<!-- The page of a long trail: its totals and a row for each stage in the map data, the stage lines on the map with a dot
     at each stage border, and the profile of the whole trail. Everything but the map comes from the trail record file; the
     map waits for the cell files of the stages, then frames their lines once. The page makes no route request. -->
<script lang="ts">
    import TrailMarker from '../components/planner/TrailMarker.svelte';
    import Detail from '../components/route/Detail.svelte';
    import ElevationRow from '../components/route/ElevationRow.svelte';
    import { distanceTicks, fitTicks } from '../components/route/ticks';
    import type { Library } from '../lib/library/store';
    import { config } from '../lib/planner/map-data';
    import { decodeCoordinates } from '../lib/planner/route-answer';
    import { catalog, type Box } from '../lib/planner/route-catalog';
    import { routeColor } from '../lib/planner/route-colors';
    import { routeWebsite } from '../lib/planner/route-overlays';
    import type { CatalogRecord } from '../lib/planner/signed-routes';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';
    import { formatDistance } from '../lib/settings/format';
    import Card from '../lib/ui/Card.svelte';
    import Lanes from '../lib/ui/Lanes.svelte';
    import TrailFacts from './explore/TrailFacts.svelte';
    import { exploreAt, signedHref } from './explore/signed-page';
    import { stageCount } from './explore/strip';
    import { borderPoints, lastEnd, lineBox, stageBorders, stageCells, stageNumbers, stageStretches, trailProfileLine, trailRows } from './explore/trail-page';
    import { signedItem } from './routes/filter';
    import RoutesMap from './routes/RoutesMap.svelte';

    let {}: { library: Library; active: boolean } = $props();

    /** The height of the profile, and the margin around the stage lines as a share of their box on each side. */
    const ELEVATION_PX = 120, MARGIN = 0.08;

    const id = $derived(Number(router.id));
    let attempt = $state(0);
    /** The trail record file of `id`: null when the release has no such trail. */
    let loaded = $state.raw<{ id: number; record?: CatalogRecord | null; failed?: boolean }>();
    const record = $derived(loaded?.id === id ? loaded.record : undefined);
    const gone = $derived(!Number.isSafeInteger(id) || record === null);
    const failed = $derived(loaded?.id === id && !!loaded.failed);
    /** The records of the stages in the catalog, when the cell files of their lines have answered. */
    let stages = $state.raw<{ trail: CatalogRecord; records: CatalogRecord[] }>();
    /** The lit part: under the pointer on its row, its line or its stretch of the profile. */
    let hovered = $state<number>(), pointer = $state<number>(), width = $state(0);

    const parts = $derived(record?.parts ?? []);
    const bike = $derived(record ? exploreAt(record).chips.bike! : 'hiking');
    const colour = $derived(routeColor(id));
    const rows = $derived(record ? trailRows(parts, record.kind, config.bounds) : []);
    const website = $derived(routeWebsite(record?.website));

    const items = $derived(stages?.trail === record ? stages!.records.map(one => signedItem(one, bike, String(id))) : []);
    const lines = $derived(new Map(stages?.trail === record ? stages!.records.map(one => [one.id, decodeCoordinates(one.line_udeg!)]) : []));
    /** The box of the stage lines with a margin that keeps them clear of the edges and the buttons of the map. It is made once:
     * the map moves only when its place changes. */
    const box = $derived.by((): Box | undefined => {
        if (!lines.size) return undefined;
        const [west, south, east, north] = lineBox([...lines.values()]), x = (east - west) * MARGIN, y = (north - south) * MARGIN;
        return [west - x, south - y, east + x, north + y];
    });
    const place = $derived(box && { lon: (box[0] + box[2]) / 2, lat: (box[1] + box[3]) / 2, box });
    const borders = $derived({ points: borderPoints(parts, one => lines.get(one)), colour });

    const total = $derived(lastEnd(parts));
    const profile = $derived(record && trailProfileLine(record));
    const stretches = $derived(stageStretches(parts));
    const borderShares = $derived(stageBorders(parts));
    const lit = $derived.by((): [number, number] | null => {
        const stretch = stretches.find(one => one.id === hovered);
        return stretch ? [stretch.from, stretch.to] : null;
    });
    const numbers = $derived(stageNumbers(stretches, width));
    const ticks = $derived(fitTicks([{ at: 0, label: 'Start' }, { at: 1, label: formatDistance(total) }, ...distanceTicks(0, total, width)], width, numbers));

    $effect(() => {
        void attempt;
        const target = id;
        loaded = undefined;
        if (!Number.isSafeInteger(target)) return;
        let live = true;
        catalog.trail(target).then(found => { if (live) loaded = { id: target, record: found }; },
            () => { if (live) loaded = { id: target, failed: true }; });
        return () => { live = false; };
    });

    // A trail that is one route record has the page of a route.
    $effect(() => {
        const href = record?.line_udeg && signedHref(record, config.bounds);
        if (href) router.replace(href);
    });

    // A cell file that fails leaves its stages out of the map.
    $effect(() => {
        const trail = record;
        stages = undefined;
        if (!trail?.parts || trail.line_udeg) return;
        const recordOf = (one: number) => catalog.record(one);
        let live = true;
        void Promise.allSettled(stageCells(trail.parts, recordOf, config.bounds).map(cell => catalog.cell(cell))).then(() => {
            if (!live) return;
            const records = trail.parts!.flatMap(part => {
                const found = part.missing ? undefined : recordOf(part.id);
                return found?.line_udeg ? [found] : [];
            });
            stages = { trail, records };
        });
        return () => { live = false; };
    });

    /** Back when Explore is the page before this one in this tab; else Explore opens on this trail in the long trails tab. */
    function toExplore(event: MouseEvent) {
        event.preventDefault();
        if (router.from === 'explore') history.back();
        else router.go(ROUTES.explore, { explore: { chips: { bike }, view: box, tab: 'long', trail: id } });
    }

    function open(next: string | undefined) {
        const found = next === undefined ? undefined : items.find(item => item.id === next);
        const href = found && signedHref(found.record, config.bounds);
        if (href) location.hash = href;
    }

    function point(at: number | null) {
        pointer = at ?? undefined;
        hovered = at === null ? undefined : stretches.find(one => at >= one.from && at <= one.to)?.id;
    }

    const message = $derived(gone ? 'This trail is no longer in the map data.' : failed ? 'The trail could not load.'
        : !record || record.line_udeg ? 'Loading…' : undefined);
</script>

<div class="trail" style:--route={colour}>
    <Detail {message} name={record?.name ?? record?.ref ?? ''} back={{ label: '‹ Explore', href: ROUTES.explore, onclick: record ? toExplore : undefined }}>
        {#snippet action()}
            {#if gone}
                <a class="action" href={ROUTES.explore} onclick={event => { event.preventDefault(); router.go(ROUTES.explore, { explore: { chips: { bike: 'hiking' }, tab: 'long' } }); }}>Show the long trails</a>
            {:else if failed}
                <button type="button" class="action" onclick={() => attempt++}>Retry</button>
            {/if}
        {/snippet}
        {#snippet mark()}{#if record}<TrailMarker symbol={record.symbol} ref={record.ref} />{/if}{/snippet}
        {#snippet tags()}
            {@const count = record && stageCount(record.stage_count, record.parts)}
            {#if count}<span class="plain">{count}</span>{/if}
        {/snippet}
        {#snippet buttons()}
            {#if website}<a class="button" href={website} target="_blank" rel="noopener noreferrer">Official page ↗</a>{/if}
        {/snippet}
        {#snippet left()}
            {#if record}<TrailFacts {record} {rows} {hovered} onhover={next => hovered = next} />{/if}
        {/snippet}
        {#snippet picture()}
            {#if stages?.trail === record}
                <div class="map">
                    <RoutesMap {items} shown={items} selected={[]} look="signed" hovered={hovered === undefined ? undefined : String(hovered)}
                        onhover={next => hovered = next === undefined ? undefined : Number(next)} onselect={open} fit={0} onview={() => {}} {place} {borders} />
                </div>
            {:else}
                <div class="waiting"></div>
            {/if}
        {/snippet}
        {#if profile}
            <Card>
                <Lanes label="Profile" {ticks} {pointer} onpointer={point} part={lit} marks={stageMarks}
                    rows={[{ label: 'Elevation', draw: elevation, height: ELEVATION_PX }]} />
            </Card>
        {/if}
    </Detail>
</div>

{#snippet elevation()}
    <div class="profile" bind:clientWidth={width}>
        <ElevationRow line={profile!} part={lit} lines={borderShares} />
    </div>
{/snippet}

{#snippet stageMarks()}
    {#each numbers as number (number.label)}
        <span class="number" style:left={`${number.at * 100}%`} style:translate={`${-number.at * 100}%`}>{number.label}</span>
    {/each}
{/snippet}

<style>
    .trail {
        height: 100%;
    }
    .trail :global(.title > :first-child:not(h1, input)) {
        flex: none;
    }
    .action {
        color: inherit;
    }
    button.action {
        padding: 0;
        border: 0;
        background: none;
        font: inherit;
        text-decoration: underline;
    }
    .map {
        min-height: 0;
        overflow: hidden;
        border: 1px solid var(--line);
        border-radius: 10px;
    }
    .waiting {
        min-height: 0;
    }
    .profile {
        height: 100%;
    }
    .number {
        position: absolute;
        color: var(--ink);
        font-weight: 600;
    }
    @media (max-width: 760px) {
        .trail {
            height: auto;
        }
        .map, .waiting {
            order: -1;
            aspect-ratio: 3 / 2;
        }
    }
</style>
