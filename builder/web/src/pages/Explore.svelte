<!-- The Explore page: the signed routes of the map view in a list with chips, on the map, and the selected one in a card
     that opens it as a plan in the planner. -->
<script lang="ts">
    import { tick, untrack } from 'svelte';
    import { keepInHistory } from '../lib/keep-in-history.svelte';
    import type { Bike } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import { config } from '../lib/planner/map-data';
    import { escapeMarkup } from '../lib/core/gpx';
    import { decodeCoordinates } from '../lib/planner/route-answer';
    import { routeColor } from '../lib/planner/route-colors';
    import { catalog, tooWide, type Box } from '../lib/planner/route-catalog';
    import { gradeName, recordCredits, routePlan, type CatalogRecord, type RouteRecord } from '../lib/planner/signed-routes';
    import { hasTrailView, loadTrailView } from '../lib/planner/trailview';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';
    import { distanceIn, heightIn } from '../lib/settings/format';
    import { today, type View } from '../lib/trailview/view';
    import Icon from '../lib/ui/Icon.svelte';
    import type { TrailViewRoute } from '../components/planner/TrailViewWindow.svelte';
    import SignedCard, { signedNumbers } from './explore/SignedCard.svelte';
    import SignedPicture from './explore/SignedPicture.svelte';
    import { firstRows, ROWS } from './explore/rows';
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

    /** The map gives its view when it is still for 100 ms; the page loads the files of a view this much later. */
    const WAIT_MS = 200;
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
    /** The answer is for another view: until the load of the new view ends, the list shows its routes in the new view. */
    const stale = $derived(!!answer && answer.view !== view);
    // Each item is for `bike`, and each item of an answer for this view is in the view.
    const inView = $derived(stale ? results(items, {}, '', view) : items);
    const shown = $derived(results(inView, chips, '', undefined, 'shortest') as SignedItem[]);
    // The map credits are HTML, and a record credit is plain text.
    const credits = $derived(recordCredits((inView as SignedItem[]).map(item => item.record)).map(escapeMarkup));
    const wide = $derived(!!view && tooWide(view));
    const chosen = $derived(shown.find(item => item.id === selected));
    /** The pictures that show a cover, and the credits of the set: the map shows them while a cover shows. */
    let covers = $state(0), coverCredits = $state.raw<string[]>([]);
    function oncover(credits: string[] | undefined) {
        if (credits) coverCredits = credits;
        covers += credits ? 1 : -1;
    }
    /** The value of each item that the other chips keep, for the bars of the slider of `key`. */
    const values = (key: 'length' | 'climb') => results(inView, { ...chips, [key]: undefined }, '')
        .map(({ summary }) => key === 'length' ? distanceIn(summary!.lengthM) : heightIn(summary!.climbM));
    const count = (n: number) => `${n.toLocaleString('en')} signed route${n === 1 ? '' : 's'}`;
    /** No row to show yet: the first load, or a new view with no route of the last answer in it. */
    const waiting = $derived(!failed && !wide && (!records || (!inView.length && stale)));
    const listed = $derived(!failed && !wide && !waiting && inView.length > 0);
    const status = $derived(failed ? 'The routes could not load.' : wide ? 'Zoom in to see the signed routes.' : waiting || stale ? 'Loading the routes…'
        : !inView.length ? 'No signed route in this map view.'
        : shown.length === inView.length ? `${count(inView.length)} in this map view` : `${shown.length.toLocaleString('en')} of ${count(inView.length)}`);

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
    /** A view with the numbers of the last one, as after a resize to the same size, is no new view: a new view starts
     * the list again. */
    function setView(next: Box) {
        // A caller can be inside an effect of the map: this read must not make that effect depend on the view.
        const now = untrack(() => view);
        if (!now || next.some((value, i) => value !== now[i])) view = next;
    }

    // A view whose cell files the catalog holds gets its answer before the page draws the view, so the list shows no
    // load. Another view loads its files `WAIT_MS` later.
    $effect.pre(() => {
        const box = view, kind = bike;
        if (!box) return;
        const held = catalog.inBoxNow(box, kind);
        if (held !== undefined) {
            loads++;
            failed = false;
            answer = { bike: kind, view: box, records: held };
            return;
        }
        const timer = setTimeout(() => void load(box, kind), WAIT_MS);
        return () => clearTimeout(timer);
    });

    // The list draws its first rows, and more when the rider scrolls near their end, so a new view makes few rows.
    let rows = $state<HTMLElement>(), rowLimit = $state(ROWS);
    const drawn = $derived(shown.length > rowLimit ? shown.slice(0, rowLimit) : shown);
    const chosenAt = $derived(chosen ? shown.indexOf(chosen) : -1);
    function drawMore() {
        if (rows && rowLimit < shown.length && rows.scrollTop + rows.clientHeight > rows.scrollHeight - 600) rowLimit += ROWS;
    }
    // A new list, from an answer or from the chips, starts at its top with its first rows.
    $effect.pre(() => {
        void [answer, chips];
        rowLimit = firstRows(untrack(() => chosenAt));
        if (rows) rows.scrollTop = 0;
    });
    // The selected route has its row and shows it: after a selection on the map or in TrailView, and in a new list,
    // which can move the route. Only the list scrolls, not the page.
    $effect(() => {
        void [answer, chips];
        const at = chosenAt;
        if (at < 0) return;
        untrack(() => { rowLimit = Math.max(rowLimit, at + 1); });
        void tick().then(() => {
            const row = rows?.querySelector('[aria-current="true"]');
            if (!row) return;
            const box = rows!.getBoundingClientRect(), at = row.getBoundingClientRect();
            if (at.top < box.top) rows!.scrollTop -= box.top - at.top;
            else if (at.bottom > box.bottom) rows!.scrollTop += at.bottom - box.bottom;
        });
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
    const trailViewTitle = $derived(shown.length > TRAILVIEW_ROUTES ? `TrailView: the first ${TRAILVIEW_ROUTES} of ${shown.length.toLocaleString('en')} routes` : undefined);

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
                    id, line: decodeCoordinates(record.line_udeg ?? []), colour: routeColor(id), credit: record.credit,
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

<ListAndMap title="Explore" {items} {shown} {place} fit={0} look="signed" credits={covers ? [...coverCredits, ...credits] : credits} selected={chosen ? [chosen.id] : []}
    onselect={id => selected = id} onview={next => setView(wrapped(next))} card={chosen ? card : undefined}
    mapButtons={hasTrailView(config) && shown.length ? mapButtons : undefined}>
    {#snippet list()}
        <!-- The search field finds places only; a chosen place moves the map. -->
        <PlaceSearch {view} label="Find a place" placeholder="Find a place" float onchoose={({ lon, lat, box }) => place = { lon, lat, box }} />
        <Chips bind:chips={() => chips, setChips} {view} keys={['bike', 'length', 'climb', ...graded ? ['grade' as const] : [], 'shape']} fixed={['bike']}
            grades={bike === 'mtb' ? [0, 1, 2, 3].map(grade => gradeName('mtb', grade)) : [0, 1, 2, 3, 4, 5].map(grade => gradeName('hiking', grade))} {values} />
        <p class={listed ? 'count' : 'note'} aria-live="polite">
            {status}{#if failed}{' '}<button type="button" class="retry" onclick={() => { if (view) void load(view, bike); }}>Retry</button>{/if}
        </p>
        {#if waiting}
            <ul class="waiting" aria-hidden="true">
                {#each { length: 5 }}<li><span class="thumb"></span><span class="lines"></span></li>{/each}
            </ul>
        {:else if listed}
            <!-- The rows of the last view stay until the answer for this view comes. -->
            <ul bind:this={rows} onscroll={drawMore}>
                {#each drawn as item (item.id)}
                    <li>
                        <RouteRow name={item.name} current={item.id === selected} onclick={() => selected = item.id} parts={signedNumbers(item.record)}>
                            {#snippet picture()}<SignedPicture {item} {oncover} small />{/snippet}
                        </RouteRow>
                    </li>
                {:else}
                    <li class="note">No route matches. Remove a chip.</li>
                {/each}
            </ul>
        {/if}
    {/snippet}
</ListAndMap>

{#snippet card(refocus: () => void)}
    <SignedCard item={chosen!} {oncover} onplan={() => plan(chosen!)} onclose={() => { refocus(); selected = undefined; }} />
{/snippet}

{#snippet mapButtons()}
    <!-- In the look of the map's own controls, beside its zoom buttons. -->
    <div class="maplibregl-ctrl maplibregl-ctrl-group">
        <button type="button" class="trailview" bind:this={trailViewButton} aria-busy={opening} aria-disabled={opening} title={trailViewTitle} onclick={openTrailView}>
            {#if opening}<span class="spinner" aria-hidden="true"></span>{:else}<Icon name="binoculars" size={18} />{/if}TrailView
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
    /* The map's controls keep their light look in both themes; this is the colour of their icons and text. */
    .trailview {
        display: flex;
        align-items: center;
        gap: 6px;
        width: auto;
        padding: 0 10px 0 8px;
        color: #333;
        font: 600 13px/1 var(--sans);
        transition: transform 160ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    .trailview:active {
        transform: scale(0.97);
    }
    /* Not `disabled` while it loads, so that the button keeps the focus. */
    .trailview[aria-disabled="true"] {
        cursor: progress;
    }
    @media (pointer: coarse) {
        .trailview {
            height: 40px;
        }
    }
    /* With its margin, the spinner has the box of the icon. */
    .spinner {
        width: 14px;
        height: 14px;
        margin: 2px;
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
    /* Under the zoom buttons of the map, as wide as they and the control together: the credits of the map end before
       the control, so the notice is never over the open credits. The numbers are the zoom buttons of MapLibre: 59 px
       high and 29 px wide, with margins of 10 px. */
    .trailview-notice {
        position: absolute;
        top: 65px;
        right: -39px;
        left: 0;
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
        padding: 4px 0 0;
        overflow: auto;
        border-top: 1px solid var(--line);
        list-style: none;
    }
    /* A row outside the list box skips its layout and paint. It keeps the height that it had, else that of a name of one line. */
    li {
        content-visibility: auto;
        contain-intrinsic-size: auto 60px;
    }
    li.note {
        padding: 6px;
    }
    .waiting {
        overflow: hidden;
    }
    .waiting li {
        display: grid;
        grid-template-columns: 72px 1fr;
        align-items: center;
        gap: 12px;
        padding: 6px;
    }
    .waiting .thumb {
        height: 48px;
        border-radius: 6px;
        background: var(--parchment-2);
    }
    /* Two bars, where the name and the numbers of a row are. */
    .waiting .lines {
        height: 30px;
        background: linear-gradient(var(--parchment-2), var(--parchment-2)) 0 0 / 62% 12px no-repeat,
            linear-gradient(var(--parchment-2), var(--parchment-2)) 0 100% / 38% 10px no-repeat;
    }
    @media (max-width: 760px) {
        ul {
            max-height: 60vh;
        }
        .waiting li {
            grid-template-columns: 64px 1fr;
            gap: 10px;
        }
        .waiting .thumb {
            height: 44px;
        }
    }
</style>
