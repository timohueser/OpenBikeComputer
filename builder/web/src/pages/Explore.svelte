<!-- The Explore page: the signed routes of the map view in a list with chips, on the map, and the selected one in a card
     that opens it as a plan in the planner. -->
<script lang="ts">
    import CoverPicture from '../lib/covers/CoverPicture.svelte';
    import { keepInHistory } from '../lib/keep-in-history.svelte';
    import type { Bike } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import { catalog, tooWide, type Box } from '../lib/planner/route-catalog';
    import { gradeName, routePlan, type CatalogRecord, type RouteRecord } from '../lib/planner/signed-routes';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';
    import { distanceIn, heightIn } from '../lib/settings/format';
    import SignedCard, { signedNumbers } from './explore/SignedCard.svelte';
    import Chips from './routes/Chips.svelte';
    import { results, signedItem, type Chips as ChipValues, type SignedItem } from './routes/filter';
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

    /** The planner makes the plan; "Find another route" there brings this state back. */
    function plan(item: SignedItem) {
        keep();
        const route = item.record as RouteRecord;
        router.go(`${ROUTES.planner}/new`, { signed: { route, plan: routePlan(route), back: $state.snapshot({ chips, view, selected }) } });
    }
</script>

<!-- Escape in an open window closes only the window. -->
<svelte:window onkeydown={event => { if (event.key === 'Escape' && !document.querySelector('dialog[open]')) selected = undefined; }} />

<ListAndMap title="Explore" {items} {shown} {place} fit={0} look="route" credits={[]} selected={chosen ? [chosen.id] : []}
    onselect={id => selected = id} onview={next => view = wrapped(next)} card={chosen ? card : undefined}>
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
                            {#snippet picture()}<CoverPicture summary={item.summary} />{/snippet}
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
