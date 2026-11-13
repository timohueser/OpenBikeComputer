<!-- The Routes page: the planned routes or the rides in a list with a search and chips, on a map, and the selected one in a
     card. -->
<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import { SvelteSet } from 'svelte/reactivity';
    import Card from '../lib/ui/Card.svelte';
    import Segmented from '../lib/ui/Segmented.svelte';
    import Switch from '../lib/ui/Switch.svelte';
    import Cover from '../lib/covers/Cover.svelte';
    import { initCore } from '../lib/core/bridge';
    import { groups } from '../lib/library/groups';
    import type { LibraryState, RideRecord, RouteRecord } from '../lib/library/records';
    import { yearTotals } from '../lib/library/statistics';
    import type { Library } from '../lib/library/store';
    import { trips } from '../lib/library/trips';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';
    import { formatDistance, formatHeight } from '../lib/settings/format';
    import Chips from './routes/Chips.svelte';
    import { readSearch, results, rideItem, routeItem, type Box, type Chips as ChipValues, type Item, type Naming, type RideItem } from './routes/filter';
    import { dayText, rideRows, tripDates, type Row } from './routes/rows';
    import SelectedCard from './routes/SelectedCard.svelte';
    import { deviceState, type DeviceState } from './routes/state';

    let { library }: { library: Library; active: boolean } = $props();

    /** A page that sends the rider here can give the search to start with. */
    const search = () => history.state?.search as { chips?: ChipValues; text?: string } | undefined;
    const READ_AFTER_MS = 200, KEEP_AFTER_MS = 300;
    const BADGES: Record<DeviceState, string> = { waiting: 'Waits', sent: 'On the device', changed: 'Changed' };
    const KEYS = { planned: ['bike', 'place', 'length', 'climb', 'days'], ridden: ['place', 'year', 'bike', 'length'] } as const;

    let records = $state.raw<RouteRecord[]>(), rides = $state.raw<RideRecord[]>(), apart = $state.raw<string[]>([]);
    let waiting = $state.raw<LibraryState['waiting']>(), sent = $state.raw<LibraryState['sent']>();
    let naming = $state.raw<Naming>(), failure = $state('');
    let chips = $state<ChipValues>(search()?.chips ?? {}), text = $state(search()?.text ?? '');
    let onlyView = $state(false), view = $state.raw<Box>(), selected = $state<string>(), fit = $state(0), adding = $state(false);
    /** The ride groups by ride key, once `groups` ends; until then each ride is its own group. */
    let grouped = $state.raw(new Map<string, string>()), opened = $state<string>();
    /** The rows in view or near it: only these mount their picture. */
    const near = new SvelteSet<string>();

    /** The view of the address. The page keeps it while the address goes to another page. */
    let ridden = $state(router.id === 'ridden');
    const routeItems = $derived(records?.map(record => routeItem(record, naming)) ?? []);
    const rideItems = $derived(rides?.map(rideItem) ?? []);
    const items: Item[] = $derived(ridden ? rideItems : routeItems);
    const shown = $derived(results(items, chips, text, onlyView ? view : undefined));
    const tripList = $derived(trips(rides ?? []));
    const rows = $derived(ridden ? rideRows(shown as RideItem[], tripList, grouped, new Map(routeItems.map(item => [item.id, item.name]))) : []);
    const group = $derived(rows.find(row => row.kind === 'group' && row.id === opened));
    /** The height of the selected card, which covers the bottom of the map. */
    let cardHeight = $state(0), cardCredits = $state<string[]>([]);
    /** The rows of the list: the routes, the ride rows, or the rides of the open group. */
    const listRows: Row[] = $derived(group ? group.rides.map(ride => ({ kind: 'ride', id: ride.id, name: ride.name, rides: [ride], at: ride.at }))
        : ridden ? rows : []);
    const mapShown = $derived(ridden ? listRows.flatMap(row => row.rides) : shown);
    const chosenRow = $derived(listRows.find(row => `${row.kind}:${row.id}` === selected));
    const chosen = $derived(ridden ? chosenRow?.rides[0] : shown.find(item => item.id === selected));
    const states = $derived(new Map(records?.map(record => [record.id, deviceState(record, waiting, sent)])));
    const times = $derived([...grouped.values()].reduce((counts, id) => counts.set(id, (counts.get(id) ?? 0) + 1), new Map<string, number>()));
    const count = (n: number, one: string) => `${n.toLocaleString('en')} ${one}${n === 1 ? '' : 's'}`;    const noun = $derived(ridden ? 'ride' : 'route');
    const listed = $derived((ridden ? rides : records) !== undefined);
    const years = $derived([...new Set(rideItems.map(item => new Date(item.at).getFullYear()))].sort((a, b) => b - a));
    const year = $derived(chips.year ?? new Date().getFullYear());
    const totals = $derived(yearTotals(rides ?? [], year));

    // The map fits the results when the list first shows, after a chip change and in the other view, unless it keeps its
    // view. A reload of the library after a write does not move it.
    $effect(() => {
        void JSON.stringify(chips);
        void opened;
        if (listed && !untrack(() => onlyView)) untrack(() => fit++);
    });

    // A chip, a deleted first ride or a new route of the group can end the open group.
    $effect(() => {
        if (opened && !group) opened = undefined;
    });

    // Each view has its own entry of the history, with its own search.
    $effect(() => {
        const next = router.id === 'ridden';
        if (router.route !== 'routes' || next === untrack(() => ridden)) return;
        ridden = next;
        untrack(() => { [chips, text, selected, opened] = [search()?.chips ?? {}, search()?.text ?? '', undefined, undefined]; });
    });

    // The groups need the rides and the routes; the list shows before they end.
    let grouping = 0;
    $effect(() => {
        if (!rides || !records) return;
        const run = ++grouping;
        groups(rides, records, apart).then(next => { if (run === grouping) grouped = next; },
            reason => { failure = reason instanceof Error ? reason.message : String(reason); });
    });

    // Back from another page brings the search again. WebKit refuses many history writes in a short time, so the page
    // writes once after the last key, and at once before a link changes the entry.
    let keep: (() => void) | undefined;
    $effect(() => {
        const search = $state.snapshot({ chips, text });
        keep = () => { keep = undefined; history.replaceState({ ...history.state, search }, ''); };
        // After a link wrote the search, `keep` is undefined: the timer must not write into the next entry.
        const timer = setTimeout(() => keep?.(), KEEP_AFTER_MS);
        return () => clearTimeout(timer);
    });

    async function load() {
        try {
            let taken;
            [records, rides, taken, waiting, sent] = await Promise.all([library.routes(), library.rides(), library.state('apart'),
                library.state('waiting'), library.state('sent')]);
            apart = taken ?? [];
        } catch (reason) {
            failure = reason instanceof Error ? reason.message : String(reason);
        }
    }

    onMount(() => {
        void load();
        // The planner modules wait for the map catalogue, so the list shows before the titles of plans with no name.
        Promise.all([import('../lib/planner/library'), import('../lib/planner/versions')])
            .then(([{ isPlan }, { planTitle }]) => { naming = { isPlan, planTitle }; },
                reason => { failure = reason instanceof Error ? reason.message : String(reason); });
        let timer: ReturnType<typeof setTimeout> | undefined;
        const stop = library.onChange((store, key) => {
            if (store !== 'routes' && store !== 'rides' && !(store === 'state' && ['waiting', 'sent', 'apart'].includes(key))) return;
            clearTimeout(timer);
            timer = setTimeout(load, READ_AFTER_MS);
        });
        return () => { stop(); clearTimeout(timer); reading?.abort(); };
    });

    const observer = new IntersectionObserver(entries => {
        for (const { target, isIntersecting } of entries) {
            const id = (target as HTMLElement).dataset.id!;
            if (isIntersecting) near.add(id);
            else near.delete(id);
        }
    }, { rootMargin: '200px' });
    onMount(() => () => observer.disconnect());

    function watch(node: HTMLElement) {
        observer.observe(node);
        return { destroy: () => { observer.unobserve(node); near.delete(node.dataset.id!); } };
    }

    /** Enter turns a place that the text names into a chip. */
    let reading: AbortController | undefined;
    /** Only the answer for the text that is still in the field changes the search; a failure keeps the text. */
    async function understand() {
        reading?.abort();
        const current = reading = new AbortController(), sent = text;
        failure = '';
        try {
            const read = await readSearch(sent, view ?? [-180, -85, 180, 85], AbortSignal.any([current.signal, AbortSignal.timeout(10_000)]));
            if (!current.signal.aborted && text === sent) [chips, text] = [{ ...chips, ...read.chips }, read.rest];
        } catch (reason) {
            if (!current.signal.aborted) failure = reason instanceof Error ? reason.message : String(reason);
        }
    }

    /** A click on a line selects its route, or the row that holds the ride. */
    function select(id: string | undefined) {
        const row = ridden ? listRows.find(row => row.rides.some(ride => ride.id === id)) : undefined;
        selected = row ? `${row.kind}:${row.id}` : ridden ? undefined : id;
    }

    const addFiles = () => initCore().then(() => import('./AddFiles.svelte')).then(module => module.default);
</script>

<!-- Escape in an open window closes only the window. -->
<svelte:window onclickcapture={event => { if ((event.target as Element).closest?.('a[href]')) keep?.(); }}
    onkeydown={event => { if (event.key === 'Escape' && !document.querySelector('dialog[open]')) selected = undefined; }} />

<!-- The second line of a row: a date, then the numbers. The line wraps between two numbers, never inside one. -->
{#snippet numbers(parts: (string | false)[], lead = '')}
    <span class="meta">{lead}{#each parts.filter(Boolean) as part, i (i)}{i || lead ? ' · ' : ''}<span>{part}</span>{/each}</span>
{/snippet}

<div class="routes-page">
    <header>
        <div class="title">
            <h1>Routes</h1>
            {#if records && rides}
                <Segmented label="Routes or rides" value={ridden ? 'ridden' : 'planned'}
                    options={[{ value: 'planned', label: `Planned ${records.length.toLocaleString('en')}` },
                        { value: 'ridden', label: `Ridden ${rides.length.toLocaleString('en')}` }]}
                    onChange={next => { keep?.(); location.hash = next === 'ridden' ? `${ROUTES.routes}/ridden` : ROUTES.routes; }} />
            {/if}
        </div>
        <nav class="buttons" aria-label="Routes">
            {#if !ridden}<a class="button primary" href={`${ROUTES.planner}/new`}>New route</a>{/if}
            <button type="button" class="button" onclick={() => adding = true}>Add files</button>
        </nav>
    </header>
    {#if failure}<p class="failure" role="alert">{failure}</p>{/if}

    <div class="cards">
        <div class="list">
            <Card>
                {#if !listed}
                    <p class="note">Loading…</p>
                {:else if !items.length}
                    <p class="note empty">
                        <strong>{ridden ? 'No rides yet.' : 'No routes yet.'}</strong>
                        {ridden ? 'Connect the device, or add GPX and FIT files.' : 'Plan one, or add GPX and FIT files.'}
                    </p>
                {:else}
                    <input class="search" type="search" aria-label={ridden ? 'Search your rides' : 'Search your routes'}
                        placeholder={ridden ? 'Search your rides, or a place' : 'Search your routes, or a place'}
                        bind:value={text} onkeydown={event => { if (event.key === 'Enter' && text.trim()) void understand(); }} />
                    <Chips bind:chips {view} keys={[...KEYS[ridden ? 'ridden' : 'planned']]} {years} />
                    {#if group}
                        <p class="count">
                            <button type="button" class="back" onclick={() => { opened = undefined; selected = undefined; }}>‹ All rides</button>
                            · {group.name} · {count(group.rides.length, 'ride')}
                        </p>
                    {:else if text.trim() || Object.keys(chips).length || onlyView}
                        <!-- The count of a search. With no search the switch above has the count. -->
                        <p class="count">{shown.length === items.length ? count(items.length, noun) : `${shown.length.toLocaleString('en')} of ${count(items.length, noun)}`}</p>
                    {/if}
                    <ul>
                        {#if ridden}
                            {#each listRows as row (`${row.kind}:${row.id}`)}
                                {@const first = row.rides[0]}
                                {@const key = `${row.kind}:${row.id}`}
                                <li data-id={key} use:watch>
                                    <button type="button" class="row" aria-current={key === selected} onclick={() => selected = key}>
                                        <span class="thumb">{#if near.has(key)}<Cover {library} id={first.id} />{/if}</span>
                                        <span class="text">
                                            <strong>{row.name}</strong>
                                            {#if row.kind === 'trip'}
                                                {@render numbers([count(row.days!, 'day'), formatDistance(row.rides.reduce((sum, ride) => sum + ride.summary.lengthM, 0))], tripDates(row))}
                                            {:else if row.kind === 'group'}
                                                <span class="meta"><span>{formatDistance(first.summary.lengthM)}</span> · last on {dayText(first.at)}</span>
                                            {:else}
                                                {@render numbers([formatDistance(first.summary.lengthM), `${formatHeight(first.summary.climbM)} up`], dayText(first.at))}
                                            {/if}
                                        </span>
                                        <!-- The room of the button that opens the group, which lies on the row. -->
                                        {#if row.kind === 'group'}<span class="open room" aria-hidden="true">{count(row.rides.length, 'ride')} ›</span>{/if}
                                    </button>
                                    {#if row.kind === 'group'}
                                        <button type="button" class="open" onclick={() => { opened = row.id; selected = undefined; }}>{count(row.rides.length, 'ride')} ›</button>
                                    {/if}
                                </li>
                            {:else}
                                <li class="note">No ride matches. Remove a chip or change the text.</li>
                            {/each}
                        {:else}
                            {#each shown as item (item.id)}
                                {@const state = states.get(item.id)}
                                <li data-id={item.id} use:watch>
                                    <button type="button" class="row" aria-current={item.id === selected} onclick={() => selected = item.id}>
                                        <span class="thumb">{#if item.summary && near.has(item.id)}<Cover {library} id={item.id} />{/if}</span>
                                        <span class="text">
                                            <strong>{item.name}</strong>
                                            {#if item.kind === 'route' && item.summary}
                                                {@const summary = item.summary}
                                                {@render numbers([summary.days > 1 && `${summary.days} days`, formatDistance(summary.lengthM), `${formatHeight(summary.climbM)} up`])}
                                            {/if}
                                        </span>
                                        {#if state}<span class="badge {state}">{BADGES[state]}</span>{/if}
                                    </button>
                                </li>
                            {:else}
                                <li class="note">No route matches. Remove a chip or change the text.</li>
                            {/each}
                        {/if}
                    </ul>
                    <div class="switch"><Switch bind:checked={onlyView} label="Only in this map view" /></div>
                    {#if ridden}
                        <p class="totals"><strong>{year}</strong> · {[formatDistance(totals.lengthM), count(totals.rides, 'ride'),
                            totals.hikes && count(totals.hikes, 'hike')].filter(Boolean).join(' · ')}</p>
                    {/if}
                {/if}
            </Card>
        </div>
        <div class="map">
            {#await import('./routes/RoutesMap.svelte') then { default: RoutesMap }}
                <!-- A route or a ride that the chips hide is not drawn as selected. -->
                <RoutesMap {items} shown={mapShown} selected={ridden ? chosenRow?.rides.map(ride => ride.id) ?? [] : chosen ? [chosen.id] : []} {fit}
                    covered={chosen && cardHeight ? cardHeight + 12 : 0} credits={chosen ? cardCredits : []}
                    look={ridden ? 'ride' : 'route'} onselect={select} onview={next => view = next} />
            {:catch error}
                <p class="note">{error instanceof Error ? error.message : String(error)}</p>
            {/await}
            {#if chosen}
                <div class="selected" bind:offsetHeight={cardHeight}>
                    <SelectedCard {library} item={chosen} row={chosenRow} device={states.get(chosen.id)} inGroup={!!group} bind:credits={cardCredits}
                        runs={ridden ? 0 : times.get(chosen.id)}
                        route={chosenRow?.kind === 'group' && records?.some(record => record.id === chosenRow.id) ? chosenRow.id : undefined} />
                </div>
            {/if}
        </div>
    </div>
</div>

{#if adding}
    {#await addFiles() then AddFiles}
        <AddFiles {library} onClose={() => adding = false} />
    {:catch error}
        <p class="failure" role="alert">{error instanceof Error ? error.message : String(error)}</p>
    {/await}
{/if}

<style>
    .routes-page {
        display: flex;
        flex-direction: column;
        gap: 12px;
        height: 100%;
        padding: 14px 20px 18px;
    }
    header {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        justify-content: space-between;
        gap: 10px 20px;
    }
    .title {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 10px 16px;
    }
    h1 {
        margin: 0;
        font: 700 25px/1.2 var(--sans);
    }
    .buttons {
        display: flex;
        gap: 8px;
    }
    .button {
        display: inline-flex;
        align-items: center;
        padding: 6px 12px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        text-decoration: none;
        white-space: nowrap;
    }
    .button:hover {
        background: var(--parchment-2);
    }
    .primary, .primary:hover {
        border-color: var(--rust);
        background: var(--rust);
        color: var(--cream);
    }
    .failure {
        margin: 0;
        color: var(--coral);
    }
    .cards {
        display: grid;
        flex: 1;
        grid-template-columns: 380px 1fr;
        grid-template-rows: minmax(0, 1fr);
        gap: 14px;
        min-height: 0;
    }
    .list > :global(section) {
        display: flex;
        flex-direction: column;
        gap: 10px;
        height: 100%;
        min-height: 0;
    }
    .note {
        margin: 4px 0;
        color: var(--ink-soft);
    }
    .empty {
        max-width: 26ch;
        margin: auto;
        padding: 24px 0;
        text-align: center;
    }
    .empty strong {
        display: block;
        margin-bottom: 4px;
        color: var(--ink);
        font-size: 16px;
    }
    .search {
        width: 100%;
        padding: 8px 10px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        background: var(--panel);
        color: var(--ink);
    }
    .count {
        margin: 0;
        color: var(--ink-soft);
        font-size: 13px;
    }
    ul {
        flex: 1;
        min-height: 0;
        margin: 0 -6px;
        padding: 0;
        overflow: auto;
        list-style: none;
    }
    .row {
        display: grid;
        grid-template-columns: 72px minmax(0, 1fr) auto;
        align-items: center;
        gap: 12px;
        width: 100%;
        padding: 6px;
        border: 0;
        border-radius: 8px;
        background: none;
        color: var(--ink);
        text-align: left;
    }
    .row:hover {
        background: var(--parchment);
    }
    .row[aria-current="true"] {
        background: var(--parchment-2);
    }
    .thumb {
        height: 48px;
        overflow: hidden;
        border-radius: 6px;
        background: var(--parchment-2);
    }
    .thumb :global(.cover) {
        background: none;
    }
    .text {
        display: flex;
        flex-direction: column;
        gap: 1px;
        min-width: 0;
    }
    /* The numbers are one line where they fit and wrap where they do not: none is cut. */
    .meta, .badge {
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    .badge, .meta span {
        white-space: nowrap;
    }
    /* The one state that waits for the rider to connect the device has a dot. */
    .badge.waiting::before {
        display: inline-block;
        width: 6px;
        height: 6px;
        margin-right: 5px;
        border-radius: 50%;
        background: var(--amber);
        vertical-align: 1px;
        content: '';
    }
    li {
        position: relative;
    }
    /* The button that opens a group lies on its row, over a column of the row with the same text and width. */
    .open.room {
        position: static;
        transform: none;
        visibility: hidden;
    }
    .open {
        position: absolute;
        top: 50%;
        right: 6px;
        padding: 2px 4px;
        border: 0;
        border-radius: 4px;
        background: none;
        color: var(--ink-soft);
        font-size: 12.5px;
        transform: translateY(-50%);
        white-space: nowrap;
    }
    .open:hover, .back:hover {
        background: var(--parchment-2);
    }
    .back {
        padding: 0 4px;
        border: 0;
        border-radius: 4px;
        background: none;
        color: var(--ink);
        font: inherit;
    }
    /* One line over the foot of the list: the switch, and under it the year. */
    .switch {
        padding-top: 8px;
        border-top: 1px solid var(--line);
    }
    .totals {
        margin: -4px 0 0;
        color: var(--ink-soft);
        font-size: 13px;
    }
    .totals strong {
        color: var(--ink);
    }
    .map {
        position: relative;
        overflow: hidden;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--parchment);
    }
    .map > .note {
        margin: 32px 16px;
    }
    .selected {
        position: absolute;
        z-index: 2;
        right: 12px;
        bottom: 12px;
        left: 12px;
    }
    @media (max-width: 760px) {
        .routes-page {
            height: auto;
            padding: 12px 16px;
        }
        .cards {
            grid-template-columns: 1fr;
            grid-template-rows: auto;
        }
        ul {
            max-height: 60vh;
        }
        .row {
            grid-template-columns: 64px minmax(0, 1fr) auto;
            gap: 10px;
        }
        .thumb {
            height: 44px;
        }
        .map {
            height: 70vh;
        }
    }
</style>
