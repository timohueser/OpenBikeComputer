<!-- The Routes page: the planned routes or the rides in a list with a search and chips, on a map, and the selected one in a
     card. -->
<script lang="ts">
    import { onMount, tick, untrack } from 'svelte';
    import Segmented from '../lib/ui/Segmented.svelte';
    import Switch from '../lib/ui/Switch.svelte';
    import Cover from '../lib/covers/Cover.svelte';
    import { initCore } from '../lib/core/bridge';
    import { keepInHistory } from '../lib/keep-in-history.svelte';
    import { groups } from '../lib/library/groups';
    import type { LibraryState, RideRecord, RouteRecord } from '../lib/library/records';
    import { yearTotals } from '../lib/library/statistics';
    import type { Library } from '../lib/library/store';
    import { trips } from '../lib/library/trips';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';
    import { distanceIn, formatDistance, formatHeight, heightIn } from '../lib/settings/format';
    import Chips from './routes/Chips.svelte';
    import ListAndMap from './routes/ListAndMap.svelte';
    import RouteRow from './routes/RouteRow.svelte';
    import { readSearch, results, rideItem, routeItem, type Box, type Chips as ChipValues, type Item, type Naming, type RideItem } from './routes/filter';
    import { dayText, rideRows, tripDates, type Row } from './routes/rows';
    import SelectedCard from './routes/SelectedCard.svelte';
    import Statistics from './routes/Statistics.svelte';
    import { deviceState, type DeviceState } from './routes/state';

    let { library }: { library: Library; active: boolean } = $props();

    /** A page that sends the rider here can give the search to start with. */
    const search = () => history.state?.search as { chips?: ChipValues; text?: string } | undefined;
    const READ_AFTER_MS = 200;
    const BADGES: Record<DeviceState, string> = { waiting: 'Waits', sent: 'On the device', changed: 'Changed' };
    const KEYS = { planned: ['bike', 'place', 'length', 'climb', 'days'], ridden: ['place', 'year', 'bike', 'length'] } as const;

    let records = $state.raw<RouteRecord[]>(), rides = $state.raw<RideRecord[]>(), apart = $state.raw<string[]>([]);
    let waiting = $state.raw<LibraryState['waiting']>(), sent = $state.raw<LibraryState['sent']>();
    let naming = $state.raw<Naming>(), failure = $state('');
    let chips = $state<ChipValues>(search()?.chips ?? {}), text = $state(search()?.text ?? '');
    let onlyView = $state(false), view = $state.raw<Box>(), selected = $state<string>(), fit = $state(0), adding = $state(false), statistics = $state(false);
    /** The ride groups by ride key, once `groups` ends; until then each ride is its own group. */
    let grouped = $state.raw(new Map<string, string>()), opened = $state<string>();

    /** The view of the address. The page keeps it while the address goes to another page. */
    let ridden = $state(router.id === 'ridden');
    const routeItems = $derived(records?.map(record => routeItem(record, naming)) ?? []);
    const rideItems = $derived(rides?.map(rideItem) ?? []);
    const items: Item[] = $derived(ridden ? rideItems : routeItems);
    const shown = $derived(results(items, chips, text, onlyView ? view : undefined));
    /** The value of each item that the text and the other chips keep, for the bars of the slider of `key`. */
    const values = (key: 'length' | 'climb') => results(items, { ...chips, [key]: undefined }, text, onlyView ? view : undefined)
        .flatMap(({ summary }) => summary ? [key === 'length' ? distanceIn(summary.lengthM) : heightIn(summary.climbM)] : []);
    const tripList = $derived(trips(rides ?? []));
    const rows = $derived(ridden ? rideRows(shown as RideItem[], tripList, grouped, new Map(routeItems.map(item => [item.id, item.name]))) : []);
    const group = $derived(rows.find(row => row.kind === 'group' && row.id === opened));
    let cardCredits = $state<string[]>([]);
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
    /** The year by bike, and on foot after it when the year has hikes. A year with no ride shows by bike. */
    const yearText = $derived([...totals.bike.count || !totals.foot.count ? [formatDistance(totals.bike.lengthM), count(totals.bike.count, 'ride')] : [],
        ...totals.foot.count ? [formatDistance(totals.foot.lengthM), count(totals.foot.count, 'hike')] : []]);

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

    // Back from another page brings the search again.
    const keep = keepInHistory('search', () => ({ chips, text }));

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

    /** The row of the ridden view that holds a ride. */
    const rowOf = (id: string | undefined) => ridden ? listRows.find(row => row.rides.some(ride => ride.id === id)) : undefined;

    /** A click on a line selects its route, or the row that holds the ride. */
    function select(id: string | undefined) {
        const row = rowOf(id);
        selected = row ? `${row.kind}:${row.id}` : ridden ? undefined : id;
    }

    /** The row of a trip is in the list only with no search and no open group. The list then scrolls to the row. */
    async function showTrip(id: string) {
        [chips, text, onlyView, opened, selected] = [{}, '', false, undefined, `trip:${id}`];
        await tick();
        document.querySelector(`.list li[data-id="${CSS.escape(selected!)}"]`)?.scrollIntoView({ block: 'nearest' });
    }

    /** The second line of a ride row and the room of the button that opens a group. */
    function rideLine(row: Row) {
        const first = row.rides[0];
        if (row.kind === 'trip') return { lead: tripDates(row), parts: [count(row.days!, 'day'), formatDistance(row.rides.reduce((sum, ride) => sum + ride.summary.lengthM, 0))] };
        if (row.kind === 'group') return { parts: [formatDistance(first.summary.lengthM)], tail: `last on ${dayText(first.at)}`, room: `${count(row.rides.length, 'ride')} ›` };
        return { lead: dayText(first.at), parts: [formatDistance(first.summary.lengthM), `${formatHeight(first.summary.climbM)} up`] };
    }

    const addFiles = () => initCore().then(() => import('./AddFiles.svelte')).then(module => module.default);
</script>

<!-- Escape in an open window closes only the window. -->
<svelte:window onkeydown={event => { if (event.key === 'Escape' && !document.querySelector('dialog[open]')) selected = undefined; }} />

<!-- A route or a ride that the chips hide is not drawn as selected. -->
<ListAndMap title="Routes" {failure} {items} shown={mapShown} {fit} look={ridden ? 'ride' : 'route'} credits={cardCredits}
    selected={ridden ? chosenRow?.rides.map(ride => ride.id) ?? [] : chosen ? [chosen.id] : []} together={id => rowOf(id)?.rides.map(ride => ride.id) ?? [id]} onselect={select} onview={next => view = next}
    card={chosen ? selectedCard : undefined}>
    {#snippet tabs()}
        {#if records && rides}
            <Segmented label="Routes or rides" value={ridden ? 'ridden' : 'planned'}
                options={[{ value: 'planned', label: `Planned ${records.length.toLocaleString('en')}` },
                    { value: 'ridden', label: `Ridden ${rides.length.toLocaleString('en')}` }]}
                onChange={next => { keep(); location.hash = next === 'ridden' ? `${ROUTES.routes}/ridden` : ROUTES.routes; }} />
        {/if}
    {/snippet}
    {#snippet actions()}
        {#if !ridden}<a class="button primary" href={`${ROUTES.planner}/new`}>New route</a>{/if}
        <button type="button" class="button" onclick={() => adding = true}>Add files</button>
    {/snippet}
    {#snippet list()}
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
            <!-- Each view has its own chips, so its menus start closed. -->
            {#key ridden}<Chips bind:chips {view} keys={[...KEYS[ridden ? 'ridden' : 'planned']]} {years} {values} />{/key}
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
                        <li data-id={key}>
                            <RouteRow name={row.name} current={key === selected} onclick={() => selected = key} {...rideLine(row)}>
                                {#snippet picture()}<Cover {library} id={first.id} />{/snippet}
                            </RouteRow>
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
                        {@const summary = item.kind === 'route' && item.summary}
                        <li>
                            <RouteRow name={item.name} current={item.id === selected} onclick={() => selected = item.id}
                                parts={summary ? [summary.days > 1 && `${summary.days} days`, formatDistance(summary.lengthM), `${formatHeight(summary.climbM)} up`] : undefined}
                                badge={state && { text: BADGES[state], kind: state }}>
                                {#snippet picture()}{#if item.summary}<Cover {library} id={item.id} />{/if}{/snippet}
                            </RouteRow>
                        </li>
                    {:else}
                        <li class="note">No route matches. Remove a chip or change the text.</li>
                    {/each}
                {/if}
            </ul>
            <div class="switch"><Switch bind:checked={onlyView} label="Only in this map view" /></div>
            {#if ridden}
                <button type="button" class="totals" aria-haspopup="dialog" onclick={() => statistics = true}>
                    <span class="meta"><strong>{year}</strong>{#each yearText as part, i (i)}{' · '}<span>{part}</span>{/each}</span>
                    <span aria-hidden="true">›</span>
                </button>
            {/if}
        {/if}
    {/snippet}
</ListAndMap>

{#snippet selectedCard()}
    <SelectedCard {library} item={chosen!} row={chosenRow} device={states.get(chosen!.id)} inGroup={!!group} bind:credits={cardCredits}
        runs={ridden ? 0 : times.get(chosen!.id)}
        route={chosenRow?.kind === 'group' && records?.some(record => record.id === chosenRow.id) ? chosenRow.id : undefined}
        onclose={() => selected = undefined} />
{/snippet}

{#if statistics}
    <Statistics rides={rides ?? []} trips={tripList} onClose={() => statistics = false} onTrip={showTrip} />
{/if}

{#if adding}
    {#await addFiles() then AddFiles}
        <AddFiles {library} onClose={() => adding = false} />
    {:catch error}
        <p class="failure" role="alert">{error instanceof Error ? error.message : String(error)}</p>
    {/await}
{/if}

<style>
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
    li {
        position: relative;
    }
    /* The button that opens a group lies on its row, over a column of the row with the same text, padding and font size. */
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
    /* One line over the foot of the list: the switch, and under it the year, which is the row that opens the statistics. */
    .switch {
        padding-top: 8px;
        border-top: 1px solid var(--line);
    }
    .totals {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        margin: -8px -6px -6px;
        padding: 6px;
        border: 0;
        border-radius: 8px;
        background: none;
        color: var(--ink-soft);
        text-align: left;
    }
    .totals:hover {
        background: var(--parchment);
    }
    .totals .meta {
        font-size: 13px;
    }
    .totals .meta span {
        white-space: nowrap;
    }
    .totals strong {
        color: var(--ink);
    }
    @media (max-width: 760px) {
        ul {
            max-height: 60vh;
        }
    }
</style>
