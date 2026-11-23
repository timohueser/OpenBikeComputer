<!-- The Explore page: the signed hikes of the map view, on the map, and the selected one in a card that opens it as a plan
     in the planner. One search bar, a text field and three slots, holds the search. The day routes are a search, with its
     rows in the list beside the map. With trail files, a second tab lists the long trails there. -->
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
    import { kinds, recordCredits, routePlan, type CatalogRecord, type RouteRecord, type TrailEntry } from '../lib/planner/signed-routes';
    import { hasTrailView, loadTrailView } from '../lib/planner/trailview';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';
    import { distanceIn, heightIn } from '../lib/settings/format';
    import { today, type View } from '../lib/trailview/view';
    import Icon from '../lib/ui/Icon.svelte';
    import Menu from '../lib/ui/Menu.svelte';
    import type { FieldProps, Place as NamedPlace } from '../lib/ui/search-bar/Field.svelte';
    import SearchBar, { type Slot } from '../lib/ui/search-bar/SearchBar.svelte';
    import Segmented from '../lib/ui/Segmented.svelte';
    import type { TrailViewRoute } from '../components/planner/TrailViewWindow.svelte';
    import SignedCard from './explore/SignedCard.svelte';
    import SignedPicture from './explore/SignedPicture.svelte';
    import TrailCard, { trailParts } from './explore/TrailCard.svelte';
    import { firstRows, ROWS } from './explore/rows';
    import HikePanel from './explore/HikePanel.svelte';
    import HikeRow from './explore/HikeRow.svelte';
    import Journey from './explore/Journey.svelte';
    import { counts, defaultQuery, DRIVE_KM, driveCap, find, GRADE_TOP, keepQuery, keepSearch, loadBox, openingQuery, queryParams, reachBox, readQuery, storedQuery,
        storedSearches, suggestions, wishesOf, type Loaded, type Query, type Shape, type Sort } from './explore/search';
    import { apply, changes, EXAMPLES, ignored, often, phraseTable, type Choice } from './explore/phrases';
    import { effectiveQuery, resetSlot, slotSet, slotWords, type SlotId } from './explore/slots';
    import Wishes from './explore/Wishes.svelte';
    import { addMarkerLayers, MARKER_LAYERS, mapData, markerClick, markerTip, myLocation, startAt } from './explore/search-map';
    import type { MapOverlay } from './routes/RoutesMap.svelte';
    import { hikes, LENGTH_TOP_M, SORT_WORDS, suggestionText } from './explore/words';
    import { signedHref } from './explore/signed-page';
    import { farOrder, farTrailId, findPlaces, nearTrails, PROFILE_STEP_M, results, scale, showAt, signedItem, splitDay, trailItem, type FoundPlace, type Item, type SignedItem,
        type TrailItem } from './routes/filter';
    import ListAndMap from './routes/ListAndMap.svelte';
    import RouteRow from './routes/RouteRow.svelte';

    /** What the page keeps in `history.state.explore`, so that Back and "Find another route" find it again. The page of a
     * signed route gives `show`, the id of the route record to show: the first answer decides its tab and its card. The page
     * of a trail gives the long trails tab and the trail. `query` is the search of the day routes (`queryParams`), and `shape`
     * the shape of its selected row. */
    interface Kept { query?: string; view?: Box; tab?: Tab; selected?: string; shape?: Shape; trail?: number; stage?: string; show?: number }
    type Place = { lon: number; lat: number; box?: Box };
    type Tab = 'day' | 'long';
    /** The files of a view: the records and the trails of a near view, or the trails of a far view. `trails` is null when
     * the view is too wide or the release has no trail files. The records are those of the cells of `from`, which the search
     * narrows to its reach, with their facts files when `facts`; `all` holds each trail of the far files of those cells. */
    type Answer = { bike: Bike; view: Box; from: Box | null; facts: boolean; far: boolean; records: CatalogRecord[] | null; trails: TrailEntry[] | null; all: Map<number, TrailEntry> };

    // The page reads the route catalog only.
    let {}: { library: Library; active: boolean } = $props();

    /** The map gives its view when it is still for 100 ms; the page loads the files of a view this much later. */
    const WAIT_MS = 200;
    const kept = history.state?.explore as Kept | undefined;
    const day = today();
    /** The search of the day routes: from the page address, else the history, else the last query of the browser. */
    const first = openingQuery(location.hash.includes('?') ? location.hash.slice(location.hash.indexOf('?') + 1) : undefined, kept?.query, storedQuery(), day);
    const firstView = first.view ?? kept?.view;

    /** The page opens on the day routes; only the rider changes the tab. A release with no trail files has no tabs. */
    let tab = $state<Tab>(config.trails ? kept?.tab ?? 'day' : 'day');
    /** The selected route of the day routes tab. */
    let view = $state.raw<Box | undefined>(firstView), selected = $state(kept?.selected);
    /** The place that the map shows: the kept view, a place of the search field, or the reach of the search. */
    let place = $state.raw<Place | undefined>(firstView && placeOf(firstView));
    let query = $state.raw<Query>(first.query), sort = $state<Sort>('match'), shape = $state(kept?.shape);
    /** The answer of the last load, for its bike and view. Both tabs read it, so with no start point a tab change loads nothing. */
    let answer = $state.raw<Answer>(), failed = $state(false);
    /** The selected trail of the long trails tab, and its selected stage. The trail stays through a change between the far
     * and the near view. */
    let trail = $state.raw<{ id: number; entry?: TrailEntry; record?: CatalogRecord; failed?: boolean }>(), stage = $state<string>();

    /** Explore searches hikes only: the bike kinds of the bar are greyed out. */
    const bike: Bike = 'hiking';
    /** The day routes are the search. Without facts files, "From" and "On the way" are off. */
    const searching = $derived(tab === 'day');
    const facts = $derived(searching && !!config.facts);
    /** The query that the day routes search, and the one that the bar shows: each value that is off has its default. */
    const dayQuery = $derived(effectiveQuery(query, { from: !config.facts, way: !config.facts, trails: false }, day));
    const shownOf = (next: Query) => effectiveQuery(next, tab === 'long' ? { from: true, way: true, trails: true } : { from: !config.facts, way: !config.facts, trails: false }, day);
    const shownQuery = $derived(shownOf(query));
    const start = $derived(dayQuery.start);
    /** The box whose cells the day routes load: the view, or with a start point the part of it in reach; null for none. */
    const from = $derived(searching && view ? loadBox({ start }, view) : view);
    /** The view needs more cell files than a view loads: the page lists the trails of the far files only. */
    const far = $derived(!!view && tooWide(view));
    /** The day routes have no list: their cells are too many. */
    const dayClosed = $derived(searching ? !!from && tooWide(from) : far);
    // The trails of a far answer are for each bike.
    const answered = $derived(answer && answer.far === far && (far || answer.bike === bike) && (!facts || answer.facts) ? answer : undefined);
    const recordOf = (id: number) => catalog.record(id);
    /** A place of the loaded facts files: a new answer gives a new function, so a name that waited for its file shows. */
    const placeById = $derived.by(() => {
        void answer;
        return (id: string) => catalog.place(id);
    });
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
    const stageItems = $derived(split.long.map(record => signedOf(record, String(farTrailId(record, answered!.all, recordOf)))));
    const trailItems = $derived((!answered?.trails ? [] : answered.far ? answered.trails : nearTrails(answered.trails, answered.all, split.long, recordOf))
        .filter(entry => kinds[bike].includes(entry.kind)).map(trailOf));
    /** The answer is for another view: until the load of the new view ends, each tab shows its rows in the new view. */
    const stale = $derived(!!answer && (answer.view !== view || answer.from !== from));
    const inViewOf = <T extends Item>(items: T[]) => stale ? results(items, {}, '', view) as T[] : items;
    const dayInView = $derived(inViewOf(dayItems)), longInView = $derived(inViewOf(trailItems));
    // The long trails keep the hardest grade of the search; its other values are for day routes.
    const longList = $derived((results(longInView, { bike, grade: query.grade[1] < GRADE_TOP ? query.grade[1] : undefined }, '') as TrailItem[]).sort(farOrder));
    const wide = $derived(far && (!config.trails || trailsTooWide(view!)));
    const dayReady = $derived(!!answered && !dayClosed && !(stale && !dayInView.length));
    /** The loaded day routes with their facts, and the rows of the search in their order. */
    const loaded = $derived<Loaded>({ records: dayInView.map(item => item.record), factsOf: id => catalog.factsOf(id), place: id => catalog.place(id) });
    const found = $derived(searching && dayReady ? find(dayQuery, loaded, sort) : []);
    const tally = $derived(searching && dayReady ? counts(dayQuery, loaded, found) : undefined);
    /** The rows that the other values keep, for the bars of the Length and Climb sliders. */
    const without = (key: 'length' | 'climb') => !dayQuery[key] ? found : searching && dayReady ? find({ ...dayQuery, [key]: undefined }, loaded) : [];
    const lengths = $derived(without('length').map(row => distanceIn(row.length_m)));
    const climbs = $derived(without('climb').map(row => heightIn(row.ascent_m)));
    /** For an empty list, the changes that help; the length widens by one step of its slider. */
    const helps = $derived(searching && dayReady && !found.length && loaded.records.length
        ? suggestions(dayQuery, loaded, Math.round(scale([distanceIn(LENGTH_TOP_M)]).step / distanceIn(1))) : []);
    /** The row of the selected route: in the selected shape, else in its first shape. */
    const opened = $derived(selected === undefined ? undefined
        : found.find(row => String(row.record.id) === selected && row.shape === shape) ?? found.find(row => String(row.record.id) === selected));
    /** The routes of the rows, each once, in the order of the rows. */
    const foundItems = $derived([...new Set(found.map(row => row.record))].map(record => signedOf(record)));
    const dayShown = $derived(foundItems);
    /** The rows of each tab, or undefined while the tab has no list for this view: the tab then shows no number. */
    const dayRows = $derived(searching && dayReady ? found : undefined);
    const longRows = $derived(answered && !wide && !(stale && !longInView.length) ? longList : undefined);
    const open = $derived(tab === 'day'
        ? { rows: dayRows, inView: dayInView as Item[], closed: dayClosed, zoom: 'Zoom in to search.', loading: 'Loading the hikes…', none: 'No hike in this map view.' }
        : { rows: longRows, inView: longInView as Item[], closed: wide, zoom: 'Zoom in to see the long trails.', loading: 'Loading the routes…', none: 'No long trail in this map view.' });
    const shown = $derived<(SignedItem | TrailItem)[]>(tab === 'day' ? dayShown : longList);
    const chosen = $derived(dayShown.find(item => item.id === selected));
    const staged = $derived(stage === undefined ? undefined : stageItems.find(item => item.id === stage));
    /** The id of the selected trail of the long trails tab. */
    const currentId = $derived(trail ? String(trail.id) : undefined);
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
    /** The lines of the row of a line, for the hover. In a near view, a trail row of the long trails tab draws as the
     * lines of its stages: the trail leads, so a stage line lights its trail row, and a trail has no start mark. */
    function together(id: string): string[] {
        if (tab === 'day' || far) return [id];
        const trailId = stageItems.find(item => item.id === id)?.trail ?? id;
        return [...new Set([trailId, ...mapShown.flatMap(item => item.kind === 'signed' && item.trail === trailId ? [item.id] : [])])];
    }
    // The map credits are HTML, and a record credit is plain text. In a far view, the entries of the trails on the map bring them.
    const credits = $derived(recordCredits([...mapShown.flatMap((item): { credit?: string }[] => item.kind === 'signed' ? [item.record] : item.kind === 'trail' ? [item.entry] : []),
        ...tab === 'long' && trail?.record ? [trail.record] : []]).map(escapeMarkup));
    /** The pictures that show a cover, and the credits of the set: the map shows them while a cover shows. */
    let covers = $state(0), coverCredits = $state.raw<string[]>([]);
    function oncover(credits: string[] | undefined) {
        if (credits) coverCredits = credits;
        covers += credits ? 1 : -1;
    }
    const counted = (rows: unknown[] | undefined) => rows ? ` ${rows.length.toLocaleString('en')}` : '';
    /** No row to show yet: the first load, or a new view with no row of the last answer in it. */
    const waiting = $derived(!failed && !open.closed && !open.rows);
    const listed = $derived(!failed && !open.closed && !!open.rows && open.inView.length > 0);
    // The tabs show the counts of the long trails; the day routes show theirs over the list, with "Sort by".
    const status = $derived(failed ? 'The routes could not load.' : open.closed ? open.zoom : waiting || stale ? open.loading : !open.inView.length ? open.none : '');
    const nice = $derived(wishesOf(dayQuery, 'nice'));

    // A selected route that the list does not show any more closes its card. A selection from the history waits for
    // the first answer.
    $effect(() => {
        if (searching && selected && answer && !chosen) selected = undefined;
    });
    $effect(() => {
        if (stage !== undefined && answer && !staged) stage = undefined;
    });

    // The page keeps only the answer of its last load. The records and the far files of a view load together, so the day
    // routes never show a list that is not split yet. A failed file fails the answer of both tabs.
    let loads = 0;
    async function load(box: Box, from: Box | null, kind: Bike, facts: boolean) {
        const id = ++loads, far = tooWide(box);
        failed = false;
        try {
            const near = !!from && !tooWide(from);
            const [records, trails] = await Promise.all([!from ? [] : near ? catalog.inBox(box, kind, from) : null, catalog.trailsInBox(box),
                near && from !== box ? catalog.trailsInBox(from) : undefined, facts && near ? catalog.factsInBox(from) : undefined]);
            // The far files of the cells have just loaded.
            if (id === loads) answer = { bike: kind, view: box, from, facts, far, records, trails, all: near ? catalog.trailsOfCellsNow(from) ?? new Map() : new Map() };
        } catch {
            if (id === loads) failed = true;
        }
    }
    function retry() {
        if (view && from !== undefined) void load(view, from, bike, facts);
    }
    /** The answer of a view whose files the catalog holds, else undefined. */
    function answerNow(box: Box, from: Box | null, kind: Bike, facts: boolean): Answer | undefined {
        const far = tooWide(box), trails = catalog.trailsInBoxNow(box);
        const near = !!from && !tooWide(from);
        const records = !from ? [] : near ? catalog.inBoxNow(box, kind, from) : null, all = near ? catalog.trailsOfCellsNow(from) : new Map();
        if (records === undefined || trails === undefined || all === undefined || (facts && from && !catalog.factsInBoxNow(from))) return undefined;
        return { bike: kind, view: box, from, facts, far, records, trails, all };
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
        const box = view, cells = from, kind = bike, withFacts = facts;
        if (!box || cells === undefined) return;
        const held = answerNow(box, cells, kind, withFacts);
        if (held) {
            loads++;
            failed = false;
            answer = held;
            return;
        }
        const timer = setTimeout(() => void load(box, cells, kind, withFacts), WAIT_MS);
        return () => clearTimeout(timer);
    });

    // The list draws its first rows, and more when the rider scrolls near their end, so a new view makes few rows.
    let rows = $state<HTMLElement>(), rowLimit = $state(ROWS);
    const dayDrawn = $derived(found.slice(0, rowLimit)), longDrawn = $derived(longList.slice(0, rowLimit));
    /** The index of the row of the selected route or trail of the open tab, or -1. */
    const chosenAt = $derived(tab === 'day' ? opened ? found.indexOf(opened) : -1 : longList.findIndex(item => item.id === currentId));
    function drawMore() {
        if (rows && rowLimit < (tab === 'day' ? found : longList).length && rows.scrollTop + rows.clientHeight > rows.scrollHeight - 600) rowLimit += ROWS;
    }
    // A new list starts at its top with its first rows: a new answer or tab, a new search or order of the day routes, or a
    // new grade of the long trails.
    $effect.pre(() => {
        void [answer, tab, query.grade, searching && query, sort];
        rowLimit = firstRows(untrack(() => chosenAt));
        if (rows) rows.scrollTop = 0;
    });
    // The selected route has its row and shows it: after a selection on the map or in TrailView, and in a new list,
    // which can move the route. Only the list scrolls, not the page.
    $effect(() => {
        void [answer, tab, query.grade, searching && query, sort];
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
        if (tab === 'day') return void ([selected, shape] = [id, undefined]);
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

    /** The trail button of a stage card opens the trail card; the focus moves to the new card. */
    function trailOfCard(id: number) {
        void openTrail(id);
        void tick().then(() => document.querySelector<HTMLElement>('.selected .close')?.focus());
    }

    /** A new way of getting there fits the map to the box of its reach. */
    function setQuery(change: Partial<Query>) {
        const journey = (['start', 'walk', 'car', 'drive', 'lift', 'pin'] as const).some(key => key in change && change[key] !== query[key]);
        query = { ...query, ...change };
        const box = journey ? reachBox(query, id => catalog.place(id)) : undefined;
        if (box) place = placeOf(box);
    }

    /** The next click on the map picks the start point. */
    let picking = $state(false);

    /** The slots of the search bar. "From" and "On the way" are off on the long trails tab and with no facts files. */
    const words = $derived(slotWords(shownQuery, placeById, day));
    const offText = $derived(tab === 'long' ? 'Not for long trails' : !config.facts ? 'No data for this map' : undefined);
    const slots = $derived<Slot[]>([
        { id: 'from', icon: 'locate', label: 'From', words: words.from, unset: 'Anywhere on the map', set: slotSet(shownQuery, 'from', day), off: offText },
        { id: 'hike', icon: 'hiking', label: 'Hike', words: words.hike, unset: tab === 'long' ? 'Any grade' : 'Any length and shape', set: slotSet(shownQuery, 'hike', day) },
        { id: 'way', icon: 'mountain', label: 'On the way', words: words.way, unset: 'Anything', set: slotSet(shownQuery, 'way', day), off: offText },
    ]);
    /** The open panel of the bar, and the query before the cross for its "Undo". */
    let openSlot = $state<string>(), cleared = $state.raw<Query>();
    /** "Reset" of a slot. On the long trails tab "Hike" shows only the hardest grade, so its reset keeps the other values of
     * the day search. */
    const reset = (id: SlotId, from: Query): Query => tab === 'long' ? { ...from, grade: [from.grade[0], GRADE_TOP] } : resetSlot(from, id, day);
    /** The cross resets each slot that is on: a slot that is off keeps its values for when it is on again. */
    function clear() {
        cleared = query;
        setQuery(slots.filter(slot => !slot.off).reduce((next, slot) => reset(slot.id as SlotId, next), query));
    }
    /** On the long trails tab "Hike" changes only the hardest grade, so the day search keeps its lower grade. */
    function setHike(change: Partial<Query>) {
        if (tab !== 'long' || !change.grade) return setQuery(change);
        const high = change.grade[1];
        setQuery({ grade: [Math.min(query.grade[0], high), high] });
    }
    /** The text field of the bar. Its words and its suggestions are read in the browser; only a typed name asks the server
     * for places, which move the map. */
    let searches = $state(storedSearches());
    /** The words of a choice that the open tab does not search stay in the field, with the text of what is off. */
    const off = $derived(offText ? ignored(shownOf, offText, day) : () => undefined);
    const phrases = $derived(phraseTable(loaded, off));
    /** The words of the slots of a query; a search with none is no last search. */
    const summary = (kept: Query) => Object.values(slotWords(kept, placeById, day)).flat().join(' · ');
    /** The places of the server for a name, the ones in the map view first: a new view is a new search. */
    const named = $derived.by(() => {
        const box = view ?? [-180, -85, 180, 85];
        return (name: string, signal: AbortSignal) => findPlaces(name, box, signal, true)
            .then(found => found.map(one => ({ ...one, about: [one.kind.replaceAll('_', ' '), one.region].filter(Boolean).join(' · ') })));
    });
    const field = $derived<FieldProps<Choice, FoundPlace & NamedPlace>>({
        table: phrases, label: 'Say what you want', placeholder: 'Say what you want, such as “loop with a hut”', examples: searches.length ? [] : EXAMPLES,
        // The search that the slots show is no last search.
        recent: searches.flatMap(text => {
            const kept = readQuery(text, day).query, words = summary(kept);
            return words && words !== summary(query) ? [{ text: words, run: () => setQuery(kept) }] : [];
        }),
        often: () => often(dayQuery, loaded, off), changes: choice => changes(dayQuery, choice), onvalues: send, places: named,
        placesLabel: 'Move the map to', onplace: ({ lon, lat, box }) => place = { lon, lat, box }, onsearch: () => { if (summary(query)) searches = keepSearch(query, day, searches); },
    });
    function send(choices: Choice[]) {
        setQuery(apply(query, choices));
        if (choices.some(choice => choice.here)) void locate();
    }
    /** The start point from the browser for a choice that needs one. A start or a pin wins, also one that the rider sets
     * meanwhile; with no location, "From" opens. */
    async function locate() {
        if (query.start || query.pin) return;
        const at = await myLocation();
        if (query.start || query.pin) return;
        if (typeof at === 'string') openSlot = 'from';
        else setQuery({ start: at, pin: undefined });
    }

    const pick = $derived(searching && picking
        ? { text: 'Click the map where you start. Esc cancels.', onpick: (at: [number, number]) => { picking = false; setQuery({ start: startAt(at), pin: undefined }); } }
        : undefined);
    /** The markers of the search: the row under the pointer, else the open row, shows its places. A new query or new loaded
     * routes give a new value, so the map asks for new tips. */
    const overlay = $derived.by<MapOverlay | undefined>(() => {
        if (!searching) return undefined;
        const [now, files, rows, open] = [dayQuery, loaded, found, opened];
        return {
            data: hovered => mapData(rows, now, (hovered === undefined ? undefined : rows.find(row => String(row.record.id) === hovered)) ?? open, id => catalog.place(id)),
            add: addMarkerLayers, hit: MARKER_LAYERS, tip: id => markerTip(id, now, files), onclick: id => setQuery(markerClick(id, now, id => catalog.place(id))),
        };
    });

    function placeOf(box: Box): Place {
        return { lon: (box[0] + box[2]) / 2, lat: (box[1] + box[3]) / 2, box };
    }

    // The record of the page of a signed route: in the day routes, else a trail of the far files or a stage of one.
    let show = kept?.show;
    $effect(() => {
        // An answer for another view or reach, as while a reset search loads, decides nothing.
        if (show === undefined || !answer || stale || (facts && !answer.facts)) return;
        const id = show, record = answer.records?.find(one => one.id === id), at = record && showAt(record, answer.all, recordOf);
        // A last search that leaves out a day route, also by a start point elsewhere, gives way to the default search in the
        // view of the route; without a start point, the next answer holds the route. A stage of a trail keeps the search.
        if (searching && (at ? at.tab === 'day' && !find(dayQuery, { ...loaded, records: [record!] }).length : !!(query.start || query.pin))) {
            query = defaultQuery(day);
            if (!at) return;
        }
        show = undefined;
        // By id: each cell file holds its own copy of a record.
        if (!at) return;
        tab = at.tab;
        if (at.tab === 'day') selected = at.selected;
        else void openTrail(at.trail).then(() => { if (trail?.id === at.trail) stage = at.stage; });
    });

    /** The page of a signed route. In the TrailView window the name is no link: Back from the page would start TrailView again. */
    const linkOf = (record: CatalogRecord) => trailView ? undefined : signedHref(record, config.bounds);

    // Back from another page brings the view, the search and the selection again. While the search shows, the page
    // address holds its query and the view.
    const keep = keepInHistory('explore', (): Kept => ({ query: queryParams(query, day), view, tab, selected, shape, trail: trail?.id, stage }),
        () => searching ? [ROUTES.explore, queryParams(query, day, view)].filter(Boolean).join('?') : ROUTES.explore);
    $effect(() => keepQuery(query, day));
    if (kept?.trail !== undefined) void openTrail(kept.trail).then(() => { if (trail?.id === kept.trail) stage = kept.stage; });

    // TrailView shows the first routes of the list from a view above them all.
    const TRAILVIEW_ROUTES = 100;
    let trailView = $state.raw<{ component: typeof import('../components/planner/TrailViewWindow.svelte').default; origin: [number, number]; view: View; routes: TrailViewRoute[] }>();
    /** A notice shows only in the map view that it is about. */
    let opening = $state(false), notice = $state.raw<{ text: string; view?: Box }>(), trailViewButton = $state<HTMLButtonElement>();
    const trailViewTitle = $derived(dayShown.length > TRAILVIEW_ROUTES ? `TrailView: the first ${TRAILVIEW_ROUTES} of ${dayShown.length.toLocaleString('en')} routes` : undefined);

    /** The first routes of the list, with the selected route among them, so the window can show its card on its line. */
    function trailViewItems(): SignedItem[] {
        const listed = dayShown.slice(0, TRAILVIEW_ROUTES);
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
            // The end of a pan or a new search during the load can change the list: one more try takes the new list.
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
                if (dayClosed || tab !== 'day') return;
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
        router.go(`${ROUTES.planner}/new`, { signed: { route, plan: routePlan(route), back: $state.snapshot({ query: queryParams(query, day), view, tab, selected, shape, trail: trail?.id, stage }) } });
    }
</script>

<!-- Escape in an open window closes only the window, and Escape that the search bar took closes only its panel. -->
<svelte:window onkeydown={event => { if (event.key === 'Escape' && !event.defaultPrevented && !document.querySelector('dialog[open]')) { if (picking) picking = false; else select(undefined); } }} />

<!-- TrailView shows the day routes of a near view only. -->
<ListAndMap title="Explore" items={mapItems} shown={mapShown} {place} fit={0} look="signed" credits={covers ? [...coverCredits, ...credits] : credits}
    selected={tab === 'day' ? chosen ? [chosen.id] : [] : staged ? [staged.id] : []} {marked} {together} {overlay} {pick} onselect={select} onview={next => setView(wrapped(next))}
    card={tab === 'day' ? chosen ? card : undefined : staged ? stageCard : trailAbout ? trailCard : undefined}
    mapButtons={hasTrailView(config) && tab === 'day' && dayShown.length && !dayClosed ? mapButtons : undefined} tabs={config.trails ? dayOrLong : undefined}
    bar={searchBar} {list} />

{#snippet list()}
    {#if searching && listed}
        <div class="top">
            <h2 aria-live="polite">{hikes(found.length)}</h2>
            {#if found.length}
                <Menu label="Sort by: {SORT_WORDS[sort]}" buttonClass="sort"
                    items={Object.entries(SORT_WORDS).map(([value, label]) => ({ label, checked: value === sort, run: () => sort = value as Sort }))} />
            {/if}
        </div>
    {:else}
        <p class={listed ? 'count' : 'note'} class:empty={!status} aria-live="polite">
            {status}{#if failed}{' '}<button type="button" class="retry" onclick={retry}>Retry</button>{/if}
        </p>
    {/if}
    {#if waiting}
        <ul class="waiting" aria-hidden="true">
            {#each { length: 5 }}<li><span class="thumb"></span><span class="lines"></span></li>{/each}
        </ul>
    {:else if listed}
        <!-- The rows of the last view stay until the answer for this view comes. -->
        <ul bind:this={rows} onscroll={drawMore}>
            {#if searching}
                {#each dayDrawn as row (`${row.record.id}/${row.shape}`)}
                    <li>
                        <HikeRow {row} item={signedOf(row.record)} {nice} place={placeById} current={row === opened} {oncover}
                            onclick={() => [selected, shape] = [String(row.record.id), row.shape]} />
                    </li>
                {:else}
                    <li class="none">{@render empty()}</li>
                {/each}
            {:else}
                {#each longDrawn as item (item.id)}
                    <li>
                        <RouteRow name={item.name} current={item.id === currentId} line={item.id} onclick={() => select(item.id)} parts={trailParts(item.entry)}>
                            {#snippet picture()}<SignedPicture {item} {oncover} row />{/snippet}
                        </RouteRow>
                    </li>
                {:else}
                    <li class="note">No route matches. Change the search.</li>
                {/each}
            {/if}
        </ul>
    {/if}
{/snippet}

{#snippet empty()}
    <strong>No hike fits all of this.</strong>
    {#if helps.length}
        <span>One change would help:</span>
        <span class="helps">
            {#each helps as help, i (i)}
                <button type="button" onclick={() => setQuery(help.query)}>{suggestionText(help, dayQuery, placeById)}<span class="n">{hikes(help.count)}</span></button>
            {/each}
        </span>
    {:else}
        <span>Widen the length or the climb.</span>
    {/if}
    <button type="button" class="text" onclick={() => query = defaultQuery(day)}>Reset the filters</button>
{/snippet}

{#snippet searchBar()}
    <SearchBar label="Search the hikes" {slots} {field} bind:open={openSlot} empty={!slots.some(slot => slot.set && !slot.off)} onreset={id => setQuery(reset(id as SlotId, query))}
        onclear={clear} onundo={() => { if (cleared) query = cleared; }}>
        {#snippet panel(id)}
            {#if id === 'from'}
                <Journey query={dayQuery} cap={start ? driveCap(start[1]) : DRIVE_KM.max} place={placeById} {picking}
                    onpicking={on => { picking = on; if (on) openSlot = undefined; }} onchange={setQuery} />
            {:else if id === 'hike'}
                <HikePanel query={shownQuery} trails={tab === 'long'} lifts={!!config.facts} ungraded={tally?.ungraded ?? 0} {lengths} {climbs} onchange={setHike} />
            {:else}
                <Wishes query={dayQuery} today={day} counts={tally?.wishes ?? new Map()} place={placeById} onchange={setQuery} />
            {/if}
        {/snippet}
    </SearchBar>
{/snippet}

{#snippet card(refocus: () => void)}
    <SignedCard item={chosen!} row={opened} href={linkOf(chosen!.record)} {oncover} onplan={() => plan(chosen!)} onclose={() => { refocus(); selected = undefined; }} />
{/snippet}

{#snippet dayOrLong()}
    <Segmented label="Day routes or long trails" value={tab} onChange={next => tab = next}
        options={[{ value: 'day', label: `Day routes${counted(dayRows)}` }, { value: 'long', label: `Long trails${counted(longRows)}` }]} />
{/snippet}

{#snippet stageCard(refocus: () => void)}
    <SignedCard item={staged!} href={linkOf(staged!.record)} {oncover} onplan={() => plan(staged!)} ontrail={trailOfCard} onclose={() => { refocus(); select(undefined); }} />
{/snippet}

{#snippet trailCard(refocus: () => void)}
    {#if trailRoute}
        <SignedCard item={trailRoute} href={linkOf(trailRoute.record)} {oncover} onplan={() => plan(trailRoute)} onclose={() => { refocus(); trail = undefined; }} />
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
    /* As high as the button of "Sort by", so the rows do not move when it goes. */
    .top {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        min-height: 30px;
    }
    h2 {
        margin: 0;
        font-size: 15px;
        font-weight: 700;
        white-space: nowrap;
    }
    /* Quiet buttons: the button of "Sort by" and "Reset the filters". */
    .top :global(.sort), .text {
        min-height: 30px;
        padding: 0 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        font-size: 13px;
    }
    .top :global(.sort) {
        margin-right: -8px;
    }
    .top :global(.sort:hover), .top :global(.sort[aria-expanded="true"]), .text:hover {
        background: var(--parchment-2);
        color: var(--ink);
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
    /* A row outside the list box skips its layout and paint. It keeps the height that it had, else that of its picture
       box and the 7 px padding of `RouteRow` above and below. */
    li {
        content-visibility: auto;
        contain-intrinsic-size: auto calc(var(--row-picture-height) + 14px);
    }
    li.note {
        padding: 6px;
    }
    /* The changes that help an empty list. */
    .none {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 10px;
        padding: 14px 6px;
        color: var(--ink-soft);
        content-visibility: visible;
    }
    .none strong {
        color: var(--ink);
        font-size: 15px;
    }
    .helps {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 6px;
    }
    .helps button {
        display: inline-flex;
        gap: 6px;
        min-height: 32px;
        align-items: center;
        padding: 0 12px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font-size: 13px;
        text-align: left;
    }
    .helps button:hover {
        background: var(--parchment-2);
    }
    .n {
        color: var(--ink-soft);
    }
    .text {
        margin-left: -8px;
    }
    .waiting {
        overflow: hidden;
    }
    .waiting li {
        display: grid;
        grid-template-columns: var(--row-picture-width) 1fr;
        align-items: center;
        gap: 12px;
        padding: 6px;
    }
    .waiting .thumb {
        height: var(--row-picture-height);
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
            gap: 10px;
        }
    }
</style>
