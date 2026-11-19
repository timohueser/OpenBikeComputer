<!-- The Explore page: the signed routes of the map view in a list with chips, on the map, and the selected one in a card
     that opens it as a plan in the planner. With trail files, one tab lists the day routes and one the long trails. -->
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
    import { gradeName, kinds, longTrailId, recordCredits, routePlan, type CatalogRecord, type RouteRecord, type TrailEntry } from '../lib/planner/signed-routes';
    import { hasTrailView, loadTrailView } from '../lib/planner/trailview';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';
    import { distanceIn, heightIn } from '../lib/settings/format';
    import { today, type View } from '../lib/trailview/view';
    import Icon from '../lib/ui/Icon.svelte';
    import Segmented from '../lib/ui/Segmented.svelte';
    import type { TrailViewRoute } from '../components/planner/TrailViewWindow.svelte';
    import SignedCard, { signedNumbers } from './explore/SignedCard.svelte';
    import SignedPicture from './explore/SignedPicture.svelte';
    import TrailCard, { trailParts } from './explore/TrailCard.svelte';
    import { firstRows, ROWS } from './explore/rows';
    import Chips from './routes/Chips.svelte';
    import { farOrder, nearTrails, PROFILE_STEP_M, results, signedItem, splitDay, trailItem, type Chips as ChipValues, type Item, type SignedItem, type TrailItem } from './routes/filter';
    import ListAndMap from './routes/ListAndMap.svelte';
    import PlaceSearch from './routes/PlaceSearch.svelte';
    import RouteRow from './routes/RouteRow.svelte';

    /** What the page keeps in `history.state.explore`, so that Back and "Find another route" find it again. */
    interface Kept { chips: ChipValues; view?: Box; tab?: Tab; selected?: string; trail?: number; stage?: string }
    type Place = { lon: number; lat: number; box?: Box };
    type Tab = 'day' | 'long';
    /** The files of a view: the records and the trails of a near view, or the trails of a far view. `trails` is null when
     * the view is too wide or the release has no trail files; `all` holds each trail of the far files of a near view. */
    type Answer = { bike: Bike; view: Box; far: boolean; records: CatalogRecord[] | null; trails: TrailEntry[] | null; all: Map<number, TrailEntry> };

    // The page reads the route catalog only.
    let {}: { library: Library; active: boolean } = $props();

    /** The map gives its view when it is still for 100 ms; the page loads the files of a view this much later. */
    const WAIT_MS = 200;
    const kept = history.state?.explore as Kept | undefined;

    let chips = $state<ChipValues>(kept?.chips ?? { bike: 'hiking' });
    /** The page opens on the day routes; only the rider changes the tab. A release with no trail files has no tabs. */
    let tab = $state<Tab>(config.trails ? kept?.tab ?? 'day' : 'day');
    /** The selected route of the day routes tab. */
    let view = $state.raw<Box | undefined>(kept?.view), selected = $state(kept?.selected);
    /** The place that the map shows: the kept view, or a place of the search field. */
    let place = $state.raw<Place | undefined>(kept?.view
        ? { lon: (kept.view[0] + kept.view[2]) / 2, lat: (kept.view[1] + kept.view[3]) / 2, box: kept.view } : undefined);
    /** The answer of the last load, for its bike and view. Both tabs read it, so a tab change loads nothing. */
    let answer = $state.raw<Answer>(), failed = $state(false);
    /** The selected trail of the long trails tab, and its selected stage. The trail stays through a change between the far
     * and the near view. */
    let trail = $state.raw<{ id: number; entry?: TrailEntry; record?: CatalogRecord; failed?: boolean }>(), stage = $state<string>();

    const bike = $derived(chips.bike ?? 'hiking');
    const graded = $derived(bike === 'hiking' || bike === 'mtb');
    /** The view needs more cell files than a view loads: the page lists the trails of the far files only. */
    const far = $derived(!!view && tooWide(view));
    // The trails of a far answer are for each bike.
    const answered = $derived(answer && answer.far === far && (far || answer.bike === bike) ? answer : undefined);
    const recordOf = (id: number) => catalog.record(id);
    // An item is made once for each record or trail and bike, so a new answer changes no row that it keeps.
    const made = new WeakMap<CatalogRecord | TrailEntry, Item>();
    function signedOf(record: CatalogRecord, trail?: string): SignedItem {
        let item = made.get(record) as SignedItem | undefined;
        if (item?.summary.bike !== bike || item.trail !== trail) made.set(record, item = signedItem(record, bike, trail));
        return item;
    }
    function trailOf(entry: TrailEntry): TrailItem {
        let item = made.get(entry) as TrailItem | undefined;
        if (item?.summary.bike !== bike) made.set(entry, item = trailItem(entry, bike));
        return item;
    }
    const split = $derived(answered?.records ? splitDay(answered.records, answered.all, recordOf) : { day: [], long: [] });
    const dayItems = $derived(split.day.map(record => signedOf(record)));
    /** The stages and single routes of the trails of a near view, in the colour of their trail. */
    const stageItems = $derived(split.long.map(record => signedOf(record, String(longTrailId(record, recordOf)))));
    const trailItems = $derived((!answered?.trails ? [] : answered.far ? answered.trails : nearTrails(answered.trails, answered.all, split.long, recordOf))
        .filter(entry => kinds[bike].includes(entry.kind)).map(trailOf));
    /** The answer is for another view: until the load of the new view ends, each tab shows its rows in the new view. */
    const stale = $derived(!!answer && answer.view !== view);
    const inViewOf = <T extends Item>(items: T[]) => stale ? results(items, {}, '', view) as T[] : items;
    const dayInView = $derived(inViewOf(dayItems)), longInView = $derived(inViewOf(trailItems));
    const dayList = $derived(results(dayInView, chips, '', undefined, 'shortest') as SignedItem[]);
    // A trail of 400 km matches no range of day routes: the Length and Climb chips belong to the day routes tab.
    const longList = $derived((results(longInView, { ...chips, length: undefined, climb: undefined }, '') as TrailItem[]).sort(farOrder));
    const wide = $derived(far && (!config.trails || trailsTooWide(view!)));
    /** The rows of each tab, or undefined while the tab has no list for this view: the tab then shows no number. */
    const dayRows = $derived(answered && !far && !(stale && !dayInView.length) ? dayList : undefined);
    const longRows = $derived(answered && !wide && !(stale && !longInView.length) ? longList : undefined);
    const open = $derived(tab === 'day'
        ? { rows: dayRows, inView: dayInView as Item[], closed: far, word: config.trails ? 'day route' : 'signed route' }
        : { rows: longRows, inView: longInView as Item[], closed: wide, word: 'long trail' });
    const shown = $derived<(SignedItem | TrailItem)[]>(tab === 'day' ? dayList : longList);
    const chosen = $derived(dayList.find(item => item.id === selected));
    const staged = $derived(stage === undefined ? undefined : stageItems.find(item => item.id === stage));
    /** The row of the selected route or trail of the open tab. */
    const currentId = $derived(tab === 'day' ? selected : trail ? String(trail.id) : undefined);
    const current = $derived(shown.find(item => item.id === currentId));
    /** What the trail card names: a trail restored from the history has nothing to name until its record comes. */
    const trailAbout = $derived(trail?.record ?? trail?.entry);
    /** A trail that is a route record has the card of a route. */
    const trailRoute = $derived(trail?.record?.line_udeg ? signedOf(trail.record, String(trail.id)) : undefined);
    /** The selected trail and its parts. The map marks them and does not move to them: a trail runs out of most views. With a
     * stage open, only the stage is wide. */
    const marked = $derived(tab === 'long' && !staged && trail && trailAbout ? [trail.id, ...trail.record?.parts?.map(part => part.id) ?? trail.record?.stages ?? []].map(String) : []);
    /** The lines of the open tab: in a near view, the long trails tab draws the stage lines of its trails. */
    const mapItems = $derived<Item[]>(tab === 'day' ? dayItems : far ? trailItems : stageItems);
    const mapShown = $derived.by<Item[]>(() => {
        if (tab === 'day' || far) return shown;
        const listed = new Set(longList.map(item => item.id));
        return stageItems.filter(item => listed.has(item.trail!));
    });
    // The map credits are HTML, and a record credit is plain text. A far file has no credits: the selected trail brings its own.
    const credits = $derived(recordCredits([...mapItems.flatMap(item => item.kind === 'signed' ? [item.record] : []), ...tab === 'long' && trail?.record ? [trail.record] : []])
        .map(escapeMarkup));
    /** The pictures that show a cover, and the credits of the set: the map shows them while a cover shows. */
    let covers = $state(0), coverCredits = $state.raw<string[]>([]);
    function oncover(credits: string[] | undefined) {
        if (credits) coverCredits = credits;
        covers += credits ? 1 : -1;
    }
    /** The value of each day route that the other chips keep, for the bars of the slider of `key`. */
    const values = (key: 'length' | 'climb') => results(dayInView, { ...chips, [key]: undefined }, '')
        .map(({ summary }) => key === 'length' ? distanceIn(summary!.lengthM) : heightIn(summary!.climbM));
    const count = (n: number) => `${n.toLocaleString('en')} signed route${n === 1 ? '' : 's'}`;
    const counted = (rows: Item[] | undefined) => rows ? ` ${rows.length.toLocaleString('en')}` : '';
    /** No row to show yet: the first load, or a new view with no row of the last answer in it. */
    const waiting = $derived(!failed && !open.closed && !open.rows);
    const listed = $derived(!failed && !open.closed && !!open.rows && open.inView.length > 0);
    // The tabs show the counts; a release with no tabs shows them under the chips.
    const status = $derived(failed ? 'The routes could not load.' : open.closed ? `Zoom in to see the ${open.word}s.`
        : waiting || stale ? 'Loading the routes…' : !open.inView.length ? `No ${open.word} in this map view.`
        : config.trails ? '' : shown.length === open.inView.length ? `${count(open.inView.length)} in this map view` : `${shown.length.toLocaleString('en')} of ${count(open.inView.length)}`);

    // A selected route that the list does not show any more closes its card. A selection from the history waits for
    // the first answer.
    $effect(() => {
        if (selected && answer && !chosen) selected = undefined;
    });
    $effect(() => {
        if (stage !== undefined && answer && !staged) stage = undefined;
    });

    // The page keeps only the answer of its last load. The records and the far files of a view load together, so the day
    // routes never show a list that is not split yet. A failed file fails the answer of both tabs.
    let loads = 0;
    async function load(box: Box, kind: Bike) {
        const id = ++loads, far = tooWide(box);
        failed = false;
        try {
            const [records, trails] = await Promise.all([far ? null : catalog.inBox(box, kind), catalog.trailsInBox(box)]);
            // The far files of the box have just loaded.
            if (id === loads) answer = { bike: kind, view: box, far, records, trails, all: far ? new Map() : catalog.trailsOfCellsNow(box) ?? new Map() };
        } catch {
            if (id === loads) failed = true;
        }
    }
    /** The answer of a view whose files the catalog holds, else undefined. */
    function answerNow(box: Box, kind: Bike): Answer | undefined {
        const far = tooWide(box), trails = catalog.trailsInBoxNow(box);
        const records = far ? null : catalog.inBoxNow(box, kind), all = far ? new Map() : catalog.trailsOfCellsNow(box);
        return records === undefined || trails === undefined || all === undefined ? undefined : { bike: kind, view: box, far, records, trails, all };
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
        const held = answerNow(box, kind);
        if (held) {
            loads++;
            failed = false;
            answer = held;
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
    // A new list, from an answer, the chips or the tab, starts at its top with its first rows.
    $effect.pre(() => {
        void [answer, chips, tab];
        rowLimit = firstRows(untrack(() => chosenAt));
        if (rows) rows.scrollTop = 0;
    });
    // The selected route has its row and shows it: after a selection on the map or in TrailView, and in a new list,
    // which can move the route. Only the list scrolls, not the page.
    $effect(() => {
        void [answer, chips, tab];
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

    /** Selects in the open tab. In the long trails tab, a trail row or line opens its card, and a stage line opens the card of
     * its trail, or its own card when its trail is the selected one. */
    function select(id: string | undefined) {
        if (tab === 'day') return void (selected = id);
        const item = id === undefined ? undefined : trailItems.find(item => item.id === id) ?? stageItems.find(item => item.id === id);
        if (item?.kind === 'trail') return void openTrail(item.entry.id, item.entry);
        if (!item) {
            trail = undefined;
            stage = undefined;
        } else if (trail?.id === Number(item.trail)) stage = item.id;
        else void openTrail(Number(item.trail));
    }

    /** The card shows the trail at once from a far file or a cell file, and its profile and strip when its record comes. */
    async function openTrail(id: number, entry?: TrailEntry) {
        stage = undefined;
        if (trail?.id === id) return;
        const next = trail = { id, entry, record: catalog.record(id) };
        // A trail opened from a stage or from the history has no far line yet: the far files of the view have it.
        const [record, entries] = await Promise.all([
            catalog.trail(id).catch((error: unknown) => { console.error('Trail:', error); return undefined; }),
            entry || !view ? undefined : catalog.trailsInBox(view).catch(() => null),
        ]);
        // A trail that is a route record keeps the record of its cell file: its card holds the cover by that object.
        const kept = next.record?.line_udeg ? next.record : record ?? next.record;
        const opened = { ...next, entry: entry ?? entries?.find(other => other.id === id), record: kept, failed: !record };
        // A trail with nothing to show, such as one of the history that a new release does not have, closes.
        if (trail === next) trail = opened.entry || opened.record ? opened : undefined;
    }

    /** The trail button of a route card opens the trail card in the long trails tab; the focus moves to the new card. */
    function trailOfCard(id: number) {
        // The trail card shows on the page, under the TrailView window.
        trailView = undefined;
        tab = 'long';
        void openTrail(id);
        void tick().then(() => document.querySelector<HTMLElement>('.selected .close')?.focus());
    }

    /** A new bike has other grades, so it clears the grade chip. */
    function setChips(next: ChipValues) {
        if (next.bike !== chips.bike) delete next.grade;
        chips = next;
    }

    // Back from another page brings the view, the chips and the selection again.
    const keep = keepInHistory('explore', (): Kept => ({ chips, view, tab, selected, trail: trail?.id, stage }));
    if (kept?.trail !== undefined) void openTrail(kept.trail).then(() => { stage = kept.stage; });

    // TrailView shows the first routes of the list from a view above them all.
    const TRAILVIEW_ROUTES = 100;
    let trailView = $state.raw<{ component: typeof import('../components/planner/TrailViewWindow.svelte').default; origin: [number, number]; view: View; routes: TrailViewRoute[] }>();
    /** A notice shows only in the map view that it is about. */
    let opening = $state(false), notice = $state.raw<{ text: string; view?: Box }>(), trailViewButton = $state<HTMLButtonElement>();
    const trailViewTitle = $derived(dayList.length > TRAILVIEW_ROUTES ? `TrailView: the first ${TRAILVIEW_ROUTES} of ${dayList.length.toLocaleString('en')} routes` : undefined);

    /** The first routes of the list, with the selected route among them, so the window can show its card on its line. */
    function trailViewItems(): SignedItem[] {
        const listed = dayList.slice(0, TRAILVIEW_ROUTES);
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
                // TrailView shows the day routes of a near view only.
                if (far || tab !== 'day') return;
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
        router.go(`${ROUTES.planner}/new`, { signed: { route, plan: routePlan(route), back: $state.snapshot({ chips, view, tab, selected, trail: trail?.id, stage }) } });
    }
</script>

<!-- Escape in an open window closes only the window. -->
<svelte:window onkeydown={event => { if (event.key === 'Escape' && !document.querySelector('dialog[open]')) select(undefined); }} />

<!-- TrailView shows the day routes of a near view only. -->
<ListAndMap title="Explore" items={mapItems} shown={mapShown} {place} fit={0} look="signed" credits={covers ? [...coverCredits, ...credits] : credits}
    selected={tab === 'day' ? chosen ? [chosen.id] : [] : staged ? [staged.id] : []} {marked} onselect={select} onview={next => setView(wrapped(next))}
    card={tab === 'day' ? chosen ? card : undefined : staged ? stageCard : trailAbout ? trailCard : undefined}
    mapButtons={hasTrailView(config) && tab === 'day' && dayList.length && !far ? mapButtons : undefined} tabs={config.trails ? dayOrLong : undefined}>
    {#snippet list()}
        <!-- The search field finds places only; a chosen place moves the map. -->
        <PlaceSearch {view} label="Find a place" placeholder="Find a place" float onchoose={({ lon, lat, box }) => place = { lon, lat, box }} />
        <Chips bind:chips={() => chips, setChips} {view} keys={['bike', ...tab === 'day' ? ['length' as const, 'climb' as const] : [], ...graded ? ['grade' as const] : [], 'shape']} fixed={['bike']}
            grades={bike === 'mtb' ? [0, 1, 2, 3].map(grade => gradeName('mtb', grade)) : [0, 1, 2, 3, 4, 5].map(grade => gradeName('hiking', grade))} {values} />
        <p class={listed ? 'count' : 'note'} class:empty={!status} aria-live="polite">
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
    <SignedCard item={chosen!} {oncover} onplan={() => plan(chosen!)} ontrail={config.trails ? trailOfCard : undefined} onclose={() => { refocus(); selected = undefined; }} />
{/snippet}

{#snippet dayOrLong()}
    <Segmented label="Day routes or long trails" value={tab} onChange={next => tab = next}
        options={[{ value: 'day', label: `Day routes${counted(dayRows)}` }, { value: 'long', label: `Long trails${counted(longRows)}` }]} />
{/snippet}

{#snippet stageCard(refocus: () => void)}
    <SignedCard item={staged!} {oncover} onplan={() => plan(staged!)} ontrail={trailOfCard} onclose={() => { refocus(); select(undefined); }} />
{/snippet}

{#snippet trailCard(refocus: () => void)}
    {#if trailRoute}
        <SignedCard item={trailRoute} {oncover} onplan={() => plan(trailRoute)} onclose={() => { refocus(); trail = undefined; }} />
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
    /* The tabs show the counts: the line keeps its place for the screen reader and takes no room. */
    .empty {
        display: none;
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
