<!-- The Explore page: the signed routes of the map view in a list with chips, on the map, and the selected one in a card
     that opens it as a plan in the planner. A map view that is too wide for the routes lists the long trails. -->
<script lang="ts">
    import { tick, untrack } from 'svelte';
    import { keepInHistory } from '../lib/keep-in-history.svelte';
    import type { Bike } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import { config } from '../lib/planner/map-data';
    import { escapeMarkup } from '../lib/core/gpx';
    import { decodeCoordinates } from '../lib/planner/route-answer';
    import { routeColor } from '../lib/planner/route-colors';
    import { catalog, tooWide, trailsTooWide, type Box } from '../lib/planner/route-catalog';
    import { gradeName, kinds, recordCredits, routePlan, type CatalogRecord, type RouteRecord, type TrailEntry } from '../lib/planner/signed-routes';
    import { hasTrailView, loadTrailView } from '../lib/planner/trailview';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';
    import { distanceIn, heightIn } from '../lib/settings/format';
    import { today, type View } from '../lib/trailview/view';
    import Icon from '../lib/ui/Icon.svelte';
    import type { TrailViewRoute } from '../components/planner/TrailViewWindow.svelte';
    import SignedCard, { signedNumbers } from './explore/SignedCard.svelte';
    import SignedPicture from './explore/SignedPicture.svelte';
    import TrailCard, { trailParts } from './explore/TrailCard.svelte';
    import { firstRows, ROWS } from './explore/rows';
    import Chips from './routes/Chips.svelte';
    import { farOrder, PROFILE_STEP_M, results, signedItem, trailItem, type Chips as ChipValues, type Item, type SignedItem, type TrailItem } from './routes/filter';
    import ListAndMap from './routes/ListAndMap.svelte';
    import PlaceSearch from './routes/PlaceSearch.svelte';
    import RouteRow from './routes/RouteRow.svelte';

    /** What the page keeps in `history.state.explore`, so that Back and "Find another route" find it again. */
    interface Kept { chips: ChipValues; view?: Box; selected?: string; trail?: number }
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
    /** The answer of the last load, for its bike and view: the records of a near view or the trails of a far view, or null
     * when the view is too wide. */
    let answer = $state.raw<{ bike: Bike; view: Box; far: boolean; found: CatalogRecord[] | TrailEntry[] | null }>(), failed = $state(false);
    /** The selected trail, apart from the selected route: it stays through a change between the far and the near view. */
    let trail = $state.raw<{ id: number; entry?: TrailEntry; record?: CatalogRecord; failed?: boolean }>();

    const bike = $derived(chips.bike ?? 'hiking');
    const graded = $derived(bike === 'hiking' || bike === 'mtb');
    /** The view needs more cell files than a view loads: the page lists the trails of the far files. */
    const far = $derived(!!view && tooWide(view));
    // The trails of a far answer are for each bike.
    const found = $derived(answer && answer.far === far && (far || answer.bike === bike) ? answer.found : undefined);
    // An item is made once for each record or trail and bike, so a new answer changes no row that it keeps.
    const made = new WeakMap<CatalogRecord | TrailEntry, Item>();
    function once<T extends Item>(key: CatalogRecord | TrailEntry, make: () => T): T {
        let item = made.get(key);
        if (item?.summary?.bike !== bike) made.set(key, item = make());
        return item as T;
    }
    const trailOf = (entry: TrailEntry) => once(entry, () => trailItem(entry, bike));
    const items = $derived<Item[]>(!found ? [] : far ? (found as TrailEntry[]).filter(entry => kinds[bike].includes(entry.kind)).map(trailOf)
        : (found as CatalogRecord[]).map(record => once(record, () => signedItem(record, bike))));
    /** The answer is for another view: until the load of the new view ends, the list shows its routes in the new view. */
    const stale = $derived(!!answer && answer.view !== view);
    // Each item is for `bike`, and each item of an answer for this view is in the view.
    const inView = $derived(stale ? results(items, {}, '', view) : items);
    // A trail of 400 km matches no range of day routes: the Length and Climb chips keep their values for the near view.
    const shown = $derived<(SignedItem | TrailItem)[]>(far ? (results(inView, { ...chips, length: undefined, climb: undefined }, '') as TrailItem[]).sort(farOrder)
        : results(inView, chips, '', undefined, 'shortest') as SignedItem[]);
    const wide = $derived(far && (!config.trails || trailsTooWide(view!)));
    const chosen = $derived(far ? undefined : shown.find(item => item.id === selected) as SignedItem | undefined);
    /** The row of the selected trail or route. */
    const currentId = $derived(trail ? String(trail.id) : selected);
    const current = $derived(shown.find(item => item.id === currentId));
    /** What the trail card names: a trail restored from the history has nothing to name until its record comes. */
    const trailAbout = $derived(trail?.record ?? trail?.entry);
    /** A trail that is a route record has the card of a route. */
    const trailRoute = $derived(trail?.record?.line_udeg ? once(trail.record, () => signedItem(trail!.record!, bike)) as SignedItem : undefined);
    /** The selected trail and its parts. The map marks them and does not move to them: a trail runs out of most views. */
    const marked = $derived(trail ? [trail.id, ...trail.record?.parts?.map(part => part.id) ?? trail.record?.stages ?? []].map(String) : []);
    // The map credits are HTML, and a record credit is plain text. A far file has no credits: the selected trail brings its own.
    const credits = $derived(recordCredits([...inView.flatMap(item => item.kind === 'signed' ? [item.record] : []), ...trail?.record ? [trail.record] : []])
        .map(escapeMarkup));
    /** The value of each item that the other chips keep, for the bars of the slider of `key`. */
    const values = (key: 'length' | 'climb') => results(inView, { ...chips, [key]: undefined }, '')
        .map(({ summary }) => key === 'length' ? distanceIn(summary!.lengthM) : heightIn(summary!.climbM));
    const count = (n: number) => `${n.toLocaleString('en')} ${far ? 'long trail' : 'signed route'}${n === 1 ? '' : 's'}`;
    /** No row to show yet: the first load, or a new view with no route of the last answer in it. */
    const waiting = $derived(!failed && !wide && (!found || (!inView.length && stale)));
    const listed = $derived(!failed && !wide && !waiting && inView.length > 0);
    const status = $derived(failed ? 'The routes could not load.' : wide ? `Zoom in to see the ${config.trails ? 'long trails' : 'signed routes'}.`
        : waiting || stale ? 'Loading the routes…' : !inView.length ? `No ${far ? 'long trail' : 'signed route'} in this map view.`
        : shown.length === inView.length ? `${count(inView.length)} in this map view` : `${shown.length.toLocaleString('en')} of ${count(inView.length)}`);

    // A selected route that the list does not show any more closes its card. A selection from the history waits for
    // the first answer.
    $effect(() => {
        if (selected && answer && !chosen) selected = undefined;
    });

    // The page keeps only the answer of its last load.
    let loads = 0;
    async function load(box: Box, kind: Bike) {
        const id = ++loads, far = tooWide(box);
        failed = false;
        try {
            const found = far ? await catalog.trailsInBox(box) : await catalog.inBox(box, kind);
            if (id === loads) answer = { bike: kind, view: box, far, found };
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

    // A view whose cell files or far files the catalog holds gets its answer before the page draws the view, so the list shows no
    // load. Another view loads its files `WAIT_MS` later.
    $effect.pre(() => {
        const box = view, kind = bike;
        if (!box) return;
        const far = tooWide(box), held = far ? catalog.trailsInBoxNow(box) : catalog.inBoxNow(box, kind);
        if (held !== undefined) {
            loads++;
            failed = false;
            answer = { bike: kind, view: box, far, found: held };
            return;
        }
        const timer = setTimeout(() => void load(box, kind), WAIT_MS);
        return () => clearTimeout(timer);
    });

    // The list draws its first rows, and more when the rider scrolls near their end, so a new view makes few rows.
    let rows = $state<HTMLElement>(), rowLimit = $state(ROWS);
    const drawn = $derived(shown.length > rowLimit ? shown.slice(0, rowLimit) : shown);
    const chosenAt = $derived(current ? shown.indexOf(current) : -1);
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

    /** A trail opens its card; a route clears the selected trail. */
    function select(id: string | undefined) {
        const item = far ? shown.find(item => item.id === id) : undefined;
        if (item?.kind === 'trail') return void openTrail(item.entry.id, item.entry);
        trail = undefined;
        selected = id;
    }

    /** The card shows the trail at once from a far file or a cell file, and its profile and strip when its record comes. */
    async function openTrail(id: number, entry?: TrailEntry) {
        selected = undefined;
        if (trail?.id === id) return;
        const next = trail = { id, entry, record: catalog.record(id) };
        let opened: typeof trail = { ...next, failed: true };
        try {
            // A trail opened from a stage or from the history has no far line yet: the far files of the view have it.
            const [record, entries] = await Promise.all([catalog.trail(id), entry || !view ? undefined : catalog.trailsInBox(view).catch(() => null)]);
            opened = { ...next, entry: entry ?? entries?.find(other => other.id === id), record: record ?? next.record, failed: !record };
        } catch (error) {
            console.error('Trail:', error);
        }
        // A trail with nothing to show, such as one of the history that a new release does not have, closes.
        if (trail === next) trail = opened.entry || opened.record ? opened : undefined;
    }

    /** The button that opens the trail card goes with the stage card: the focus moves to the new card. */
    function trailOfCard(id: number) {
        void openTrail(id);
        void tick().then(() => document.querySelector<HTMLElement>('.selected .close')?.focus());
    }

    /** A new bike has other grades, so it clears the grade chip. */
    function setChips(next: ChipValues) {
        if (next.bike !== chips.bike) delete next.grade;
        chips = next;
    }

    // Back from another page brings the view, the chips and the selection again.
    const keep = keepInHistory('explore', (): Kept => ({ chips, view, selected, trail: trail?.id }));
    if (kept?.trail !== undefined) void openTrail(kept.trail);

    // TrailView shows the first routes of the list from a view above them all.
    const TRAILVIEW_ROUTES = 100;
    let trailView = $state.raw<{ component: typeof import('../components/planner/TrailViewWindow.svelte').default; origin: [number, number]; view: View; routes: TrailViewRoute[] }>();
    /** A notice shows only in the map view that it is about. */
    let opening = $state(false), notice = $state.raw<{ text: string; view?: Box }>(), trailViewButton = $state<HTMLButtonElement>();
    const trailViewTitle = $derived(shown.length > TRAILVIEW_ROUTES ? `TrailView: the first ${TRAILVIEW_ROUTES} of ${shown.length.toLocaleString('en')} routes` : undefined);

    /** The first routes of the list, with the selected route among them, so the window can show its card on its line. */
    function trailViewItems(): SignedItem[] {
        const listed = (shown as SignedItem[]).slice(0, TRAILVIEW_ROUTES);
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
        router.go(`${ROUTES.planner}/new`, { signed: { route, plan: routePlan(route), back: $state.snapshot({ chips, view, selected, trail: trail?.id }) } });
    }
</script>

<!-- Escape in an open window closes only the window. -->
<svelte:window onkeydown={event => { if (event.key === 'Escape' && !document.querySelector('dialog[open]')) select(undefined); }} />

<!-- TrailView shows the routes of a near view only. -->
<ListAndMap title="Explore" {items} {shown} {place} fit={0} look="signed" {credits} selected={chosen ? [chosen.id] : []} {marked}
    onselect={select} onview={next => setView(wrapped(next))} card={trailAbout ? trailCard : chosen ? card : undefined}
    mapButtons={hasTrailView(config) && shown.length && !far ? mapButtons : undefined}>
    {#snippet list()}
        <!-- The search field finds places only; a chosen place moves the map. -->
        <PlaceSearch {view} label="Find a place" placeholder="Find a place" float onchoose={({ lon, lat, box }) => place = { lon, lat, box }} />
        <Chips bind:chips={() => chips, setChips} {view} keys={['bike', ...far ? [] : ['length' as const, 'climb' as const], ...graded ? ['grade' as const] : [], 'shape']} fixed={['bike']}
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
                        <RouteRow name={item.name} current={item.id === currentId} onclick={() => select(item.id)}
                            parts={item.kind === 'trail' ? trailParts(item.entry) : signedNumbers(item.record)}>
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

{#snippet card(refocus: () => void)}
    <SignedCard item={chosen!} onplan={() => plan(chosen!)} ontrail={config.trails ? trailOfCard : undefined} onclose={() => { refocus(); selected = undefined; }} />
{/snippet}

{#snippet trailCard(refocus: () => void)}
    {#if trailRoute}
        <SignedCard item={trailRoute} onplan={() => plan(trailRoute)} onclose={() => { refocus(); trail = undefined; }} />
    {:else if trailAbout}
        <TrailCard about={trailAbout} item={trail!.entry && trailOf(trail!.entry)} record={trail!.record} failed={!!trail!.failed} onclose={() => { refocus(); trail = undefined; }} />
    {/if}
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
    <trailView.component origin={trailView.origin} view={trailView.view} routes={trailView.routes} {selected} onselect={select}
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
