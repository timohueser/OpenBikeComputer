<!-- The Explore page: the signed routes of the map view in a list with chips, on the map, and the selected one in a card
     that opens it as a plan in the planner. -->
<script lang="ts">
    import { keepInHistory } from '../lib/keep-in-history.svelte';
    import type { Bike } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import { config } from '../lib/planner/map-data';
    import { decodeCoordinates } from '../lib/planner/route-answer';
    import { routeColor } from '../lib/planner/route-colors';
    import { catalog, tooWide, type Box } from '../lib/planner/route-catalog';
    import { gradeName, routePlan, type CatalogRecord, type RouteRecord } from '../lib/planner/signed-routes';
    import { hasTrailView, loadTrailView } from '../lib/planner/trailview';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';
    import { distanceIn, heightIn } from '../lib/settings/format';
    import { today, type View } from '../lib/trailview/view';
    import Icon from '../lib/ui/Icon.svelte';
    import type { TrailViewRoute } from '../components/planner/TrailViewWindow.svelte';
    import SignedCard, { signedNumbers } from './explore/SignedCard.svelte';
    import SignedPicture from './explore/SignedPicture.svelte';
    import Chips from './routes/Chips.svelte';
    import { PROFILE_STEP_M, results, signedItem, type Chips as ChipValues, type SignedItem } from './routes/filter';
    import ListAndMap from './routes/ListAndMap.svelte';
    import PlaceSearch from './routes/PlaceSearch.svelte';
    import RouteRow from './routes/RouteRow.svelte';

    /** What the page keeps in `history.state.explore`, so that Back and "Find another route" find it again. */
    interface Kept { chips: ChipValues; view?: Box; selected?: string }
    type Place = { lon: number; lat: number; box?: Box };

    // The page reads the route catalog only.
    let {}: { library: Library; active: boolean } = $props();

    const WAIT_MS = 300;
    const kept = history.state?.explore as Kept | undefined;

    let chips = $state<ChipValues>(kept?.chips ?? { bike: 'hiking' });
    let view = $state.raw<Box | undefined>(kept?.view), selected = $state(kept?.selected);
    /** The place that the map shows: the kept view, or a place of the search field. */
    let place = $state.raw<Place | undefined>(kept?.view
        ? { lon: (kept.view[0] + kept.view[2]) / 2, lat: (kept.view[1] + kept.view[3]) / 2, box: kept.view } : undefined);
    /** The answer of the last load, for its bike and view: the records, or null when the view needs too many cells. */
    let answer = $state.raw<{ bike: Bike; view: Box; records: CatalogRecord[] | null }>(), failed = $state(false);

    const bike = $derived(chips.bike ?? 'hiking');
    const graded = $derived(bike === 'hiking' || bike === 'mtb');
    const records = $derived(answer?.bike === bike ? answer.records : undefined);
    // An item is made once for each record and bike, so a new answer changes no row that it keeps.
    const made = new WeakMap<CatalogRecord, SignedItem>();
    const itemOf = (record: CatalogRecord) => {
        let item = made.get(record);
        if (item?.summary.bike !== bike) made.set(record, item = signedItem(record, bike));
        return item;
    };
    const items = $derived(records?.map(itemOf) ?? []);
    // Each item is for `bike`. Until the load of a new view ends, the items are those of the last view: the list counts
    // and shows the ones in the new view.
    const inView = $derived(results(items, {}, '', view));
    const shown = $derived(results(items, chips, '', view, 'shortest') as SignedItem[]);
    const wide = $derived(!!view && tooWide(view));
    const chosen = $derived(shown.find(item => item.id === selected));
    /** The value of each item that the other chips keep, for the bars of the slider of `key`. */
    const values = (key: 'length' | 'climb') => results(items, { ...chips, [key]: undefined }, '', view)
        .map(({ summary }) => key === 'length' ? distanceIn(summary!.lengthM) : heightIn(summary!.climbM));
    const count = (n: number) => `${n.toLocaleString('en')} signed route${n === 1 ? '' : 's'}`;

    // A selected route that the list does not show any more closes its card. A selection from the history waits for
    // the first answer.
    $effect(() => {
        if (selected && answer && !chosen) selected = undefined;
    });

    // The page keeps only the answer of its last load.
    let loads = 0;
    async function load(box: Box, kind: Bike) {
        const id = ++loads;
        failed = false;
        try {
            const found = await catalog.inBox(box, kind);
            if (id === loads) answer = { bike: kind, view: box, records: found };
        } catch {
            if (id === loads) failed = true;
        }
    }
    $effect(() => {
        const box = view, kind = bike;
        if (!box) return;
        const timer = setTimeout(() => void load(box, kind), WAIT_MS);
        return () => clearTimeout(timer);
    });

    /** The view with the longitude of its middle in -180..180, and its ends cut there, as `inBox` needs it. */
    function wrapped([west, south, east, north]: Box): Box {
        const shift = 360 * Math.round((west + east) / 720);
        return [Math.max(-180, west - shift), south, Math.min(180, east - shift), north];
    }

    /** A new bike has other grades, so it clears the grade chip. */
    function setChips(next: ChipValues) {
        if (next.bike !== chips.bike) delete next.grade;
        chips = next;
    }

    // Back from another page brings the view, the chips and the selection again.
    const keep = keepInHistory('explore', (): Kept => ({ chips, view, selected }));

    // TrailView shows the first routes of the list from a view above them all.
    const TRAILVIEW_ROUTES = 100;
    let trailView = $state.raw<{ component: typeof import('../components/planner/TrailViewWindow.svelte').default; origin: [number, number]; view: View; routes: TrailViewRoute[] }>();
    /** A notice shows only in the map view that it is about. */
    let opening = $state(false), notice = $state.raw<{ text: string; view?: Box }>(), trailViewButton = $state<HTMLButtonElement>();
    const trailViewTitle = $derived(shown.length > TRAILVIEW_ROUTES ? `TrailView: the first ${TRAILVIEW_ROUTES} of ${shown.length.toLocaleString('en')} routes` : 'TrailView');

    /** The first routes of the list, with the selected route among them, so the window can show its card on its line. */
    function trailViewItems(): SignedItem[] {
        const listed = shown.slice(0, TRAILVIEW_ROUTES);
        if (chosen && !listed.includes(chosen)) listed[listed.length - 1] = chosen;
        return listed;
    }

    async function openTrailView(event: MouseEvent) {
        if (opening) return;
        const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
        opening = true;
        notice = undefined;
        try {
            const [chunk, { default: component }] = await Promise.all([loadTrailView(), import('../components/planner/TrailViewWindow.svelte')]);
            // The end of a pan or new chips during the load can change the list: one more try takes the new list.
            for (let tries = 0; tries < 2; tries++) {
                const listed = trailViewItems();
                const routes = listed.map(({ id, record }): TrailViewRoute => ({
                    id, line: decodeCoordinates(record.line_udeg ?? []), colour: routeColor(id),
                    ...record.profile_m ? { profile: { step: PROFILE_STEP_M, heights: record.profile_m, length: record.length_m } } : {},
                }));
                // The size of the window: the rule of its CSS.
                const full = innerWidth <= 760;
                // A failure is about the routes it tried: it shows only while the list still has them.
                const first = await chunk.coverView(routes.map(route => route.line), config,
                    { width: full ? innerWidth : innerWidth - 64, height: full ? innerHeight : innerHeight - 64, date: today(), time: '10:00', weather: 'sunny' })
                    .catch((error: unknown) => { console.error('TrailView:', error); return null; });
                const now = trailViewItems();
                if (now.length !== listed.length || now.some((item, i) => item.id !== listed[i].id)) continue;
                if (first) trailView = { component, origin: [box.left + box.width / 2, box.top + box.height / 2], view: first, routes };
                else notice = { text: first === null ? 'TrailView could not start. Try again.' : 'TrailView has no view of this area.', view };
                return;
            }
            notice = { text: 'The routes changed while TrailView loaded. Try again.', view };
        } catch (error) {
            console.error('TrailView:', error);
            notice = { text: 'TrailView could not start. Try again.', view };
        } finally {
            opening = false;
        }
    }

    /** The planner makes the plan; "Find another route" there brings this state back. */
    function plan(item: SignedItem) {
        keep();
        const route = item.record as RouteRecord;
        router.go(`${ROUTES.planner}/new`, { signed: { route, plan: routePlan(route), back: $state.snapshot({ chips, view, selected }) } });
    }
</script>

<!-- Escape in an open window closes only the window. -->
<svelte:window onkeydown={event => { if (event.key === 'Escape' && !document.querySelector('dialog[open]')) selected = undefined; }} />

<ListAndMap title="Explore" {items} {shown} {place} fit={0} look="signed" credits={[]} selected={chosen ? [chosen.id] : []}
    onselect={id => selected = id} onview={next => view = wrapped(next)} card={chosen ? card : undefined}
    mapButtons={hasTrailView(config) && shown.length ? mapButtons : undefined}>
    {#snippet list()}
        <!-- The search field finds places only; a chosen place moves the map. -->
        <PlaceSearch {view} label="Find a place" placeholder="Find a place" float onchoose={({ lon, lat, box }) => place = { lon, lat, box }} />
        <Chips bind:chips={() => chips, setChips} {view} keys={['bike', 'length', 'climb', ...graded ? ['grade' as const] : [], 'shape']} fixed={['bike']}
            grades={bike === 'mtb' ? [0, 1, 2, 3].map(grade => gradeName('mtb', grade)) : [0, 1, 2, 3, 4, 5].map(grade => gradeName('hiking', grade))} {values} />
        {#if failed}
            <p class="note">The routes could not load. <button type="button" class="retry" onclick={() => { if (view) void load(view, bike); }}>Retry</button></p>
        {:else if wide}
            <p class="note">Zoom in to see the signed routes.</p>
        {:else if !records || (!inView.length && answer!.view !== view)}
            <p class="note">Loading the routes…</p>
        {:else if !inView.length}
            <p class="note">No signed route in this map view.</p>
        {:else}
            <!-- The rows of the last view stay until the answer for this view comes. -->
            <p class="count">{answer!.view !== view ? 'Loading the routes…' : shown.length === inView.length ? `${count(inView.length)} in this map view` : `${shown.length.toLocaleString('en')} of ${count(inView.length)}`}</p>
            <ul>
                {#each shown as item (item.id)}
                    <li>
                        <RouteRow name={item.name} current={item.id === selected} onclick={() => selected = item.id} parts={signedNumbers(item.record)}>
                            {#snippet picture()}<SignedPicture {item} />{/snippet}
                        </RouteRow>
                    </li>
                {:else}
                    <li class="note">No route matches. Remove a chip.</li>
                {/each}
            </ul>
        {/if}
    {/snippet}
</ListAndMap>

{#snippet card()}
    <SignedCard item={chosen!} onplan={() => plan(chosen!)} />
{/snippet}

{#snippet mapButtons()}
    <!-- A group of the map's own controls, as the zoom buttons above it. -->
    <div class="maplibregl-ctrl maplibregl-ctrl-group">
        <button type="button" class="trailview" bind:this={trailViewButton} aria-label="TrailView" aria-busy={opening} aria-disabled={opening} title={trailViewTitle} onclick={openTrailView}>
            {#if opening}<span class="spinner" aria-hidden="true"></span>{:else}<Icon name="binoculars" size={18} />{/if}
        </button>
    </div>
    {#if notice && notice.view === view}<p class="trailview-notice" role="status">{notice.text}</p>{/if}
{/snippet}

{#if trailView}
    <trailView.component origin={trailView.origin} view={trailView.view} routes={trailView.routes} {selected} onselect={id => selected = id}
        card={chosen ? card : undefined} onClose={() => { trailView = undefined; trailViewButton?.focus(); }} />
{/if}

<style>
    .note, .count {
        margin: 0;
        color: var(--ink-soft);
    }
    .count {
        font-size: 13px;
    }
    .retry {
        padding: 0 4px;
        border: 0;
        border-radius: 4px;
        background: none;
        color: var(--ink);
        font: inherit;
        text-decoration: underline;
    }
    /* The map's controls keep their light look in both themes; this is the colour of their icons. */
    .trailview {
        display: grid;
        place-items: center;
        color: #333;
    }
    /* Not `disabled` while it loads, so that the button keeps the focus. */
    .trailview[aria-disabled="true"] {
        cursor: progress;
    }
    .spinner {
        width: 14px;
        height: 14px;
        border: 2px solid color-mix(in srgb, currentColor 25%, transparent);
        border-top-color: currentColor;
        border-radius: 50%;
        animation: spin 900ms linear infinite;
    }
    @keyframes spin {
        to { transform: rotate(1turn); }
    }
    @media (prefers-reduced-motion: reduce) {
        .spinner { animation-duration: 3s; }
    }
    .trailview-notice {
        max-width: 220px;
        margin: 0;
        padding: 6px 10px;
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink-soft);
        font-size: 13px;
        box-shadow: var(--planner-shadow);
    }
    ul {
        flex: 1;
        min-height: 0;
        margin: 0 -6px;
        padding: 0;
        overflow: auto;
        list-style: none;
    }
    li.note {
        padding: 6px;
    }
    @media (max-width: 760px) {
        ul {
            max-height: 60vh;
        }
    }
</style>
