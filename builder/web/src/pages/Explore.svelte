<!-- The Explore page: the signed routes of the map view in a list with chips, on the map, and the selected one in a card
     that opens it as a plan in the planner. -->
<script lang="ts">
    import { onDestroy } from 'svelte';
    import CoverPicture from '../lib/covers/CoverPicture.svelte';
    import type { Bike } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import { catalog, type Box } from '../lib/planner/route-catalog';
    import { gradeName, routePlan, type CatalogRecord, type RouteRecord } from '../lib/planner/signed-routes';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';
    import { distanceIn, heightIn } from '../lib/settings/format';
    import SignedCard, { signedNumbers } from './explore/SignedCard.svelte';
    import Chips from './routes/Chips.svelte';
    import { findPlaces, results, signedItem, type Chips as ChipValues, type FoundPlace, type SignedItem } from './routes/filter';
    import ListAndMap from './routes/ListAndMap.svelte';
    import RouteRow from './routes/RouteRow.svelte';

    /** What the page keeps in `history.state.explore`, so that Back and "Find another route" find it again. */
    interface Kept { chips: ChipValues; view?: Box; selected?: string }
    type Place = { lon: number; lat: number; box?: Box };

    // The page reads the route catalog only.
    let {}: { library: Library; active: boolean } = $props();

    const WAIT_MS = 300;
    const kept = history.state?.explore as Kept | undefined;

    let chips = $state<ChipValues>(kept?.chips ?? { bike: (history.state?.bike as Bike | undefined) ?? 'hiking' });
    let view = $state.raw<Box | undefined>(kept?.view), selected = $state(kept?.selected);
    /** The place that the map shows: the kept view, a place that another page gives, or a place of the search field. */
    let place = $state.raw<Place | undefined>(kept?.view
        ? { lon: (kept.view[0] + kept.view[2]) / 2, lat: (kept.view[1] + kept.view[3]) / 2, box: kept.view } : history.state?.place);
    /** The answer of the last load, for its bike: the records, or null when the view needs too many cells. */
    let answer = $state.raw<{ bike: Bike; records: CatalogRecord[] | null }>(), failed = $state(false);

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
    const shown = $derived(results(items, chips, '', view, 'shortest') as SignedItem[]);
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
            const found = await catalog.inBox(wrapped(box), kind);
            if (id === loads) answer = { bike: kind, records: found };
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

    // Back from another page brings the view, the chips and the selection again. WebKit refuses many history writes in
    // a short time, so the page writes once after the last change, and at once before a link changes the entry.
    let keep: (() => void) | undefined;
    $effect(() => {
        const explore: Kept = $state.snapshot({ chips, view, selected });
        keep = () => { keep = undefined; history.replaceState({ ...history.state, explore }, ''); };
        // After a link wrote the state, `keep` is undefined: the timer must not write into the next entry.
        const timer = setTimeout(() => keep?.(), WAIT_MS);
        return () => clearTimeout(timer);
    });

    /** The planner makes the plan; "Find another route" there brings this state back. */
    function plan(item: SignedItem) {
        keep?.();
        const route = item.record as RouteRecord;
        router.go(`${ROUTES.planner}/new`, { signed: { route, plan: routePlan(route), back: $state.snapshot({ chips, view, selected }) } });
    }

    // The search field finds places only; a chosen place moves the map.
    let query = $state(''), found = $state.raw<FoundPlace[] | null>(null), findFailure = $state('');
    let timer: ReturnType<typeof setTimeout> | undefined, controller: AbortController | undefined, field = $state<HTMLElement>();
    function find() {
        clearTimeout(timer);
        controller?.abort();
        [found, findFailure] = [null, ''];
        const text = query.trim();
        if (!text) return;
        timer = setTimeout(() => {
            const current = controller = new AbortController();
            findPlaces(text, view ?? [-180, -85, 180, 85], current.signal).then(
                places => { if (!current.signal.aborted) found = places; },
                reason => { if (!current.signal.aborted) findFailure = reason instanceof Error ? reason.message : String(reason); });
        }, WAIT_MS);
    }
    function go({ name, lon, lat, box }: FoundPlace) {
        [query, found, place] = [name, null, { lon, lat, box }];
    }
    onDestroy(() => { clearTimeout(timer); controller?.abort(); });
</script>

<!-- Escape in an open window closes only the window. -->
<svelte:window onclickcapture={event => { if ((event.target as Element).closest?.('a[href]')) keep?.(); }}
    onclick={event => { if (found && !field?.contains(event.target as Node)) found = null; }}
    onkeydown={event => { if (event.key === 'Escape' && !document.querySelector('dialog[open]')) [selected, found] = [undefined, null]; }} />

<ListAndMap title="Explore" {items} {shown} {place} fit={0} look="route" credits={[]} selected={chosen ? [chosen.id] : []}
    onselect={id => selected = id} onview={next => view = next} card={chosen ? card : undefined}>
    {#snippet list()}
        <div class="where" bind:this={field}>
            <input class="search" type="search" aria-label="Find a place" placeholder="Find a place" bind:value={query} oninput={find} />
            {#if findFailure || found}
                <div class="found">
                    {#each found ?? [] as option (option.box.join() + option.name)}
                        <button type="button" onclick={() => go(option)}>{option.name}<small>{[option.kind.replaceAll('_', ' '), option.region].filter(Boolean).join(' · ')}</small></button>
                    {:else}
                        <p class="note">{findFailure || 'No place found.'}</p>
                    {/each}
                </div>
            {/if}
        </div>
        <Chips bind:chips={() => chips, setChips} {view} keys={['bike', 'length', 'climb', ...graded ? ['grade' as const] : [], 'shape']} fixed={['bike']}
            grades={[0, 1, 2, 3].map(grade => gradeName(bike === 'mtb' ? 'mtb' : 'hiking', grade))} {values} />
        {#if failed}
            <p class="note">The routes could not load. <button type="button" class="retry" onclick={() => { if (view) void load(view, bike); }}>Retry</button></p>
        {:else if records === null}
            <p class="note">Zoom in to see the signed routes.</p>
        {:else if !records}
            <p class="note">Loading the routes…</p>
        {:else if !items.length}
            <p class="note">No signed route in this map view.</p>
        {:else}
            <p class="count">{shown.length === items.length ? `${count(items.length)} in this map view` : `${shown.length.toLocaleString('en')} of ${count(items.length)}`}</p>
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
    .where {
        position: relative;
    }
    .search {
        width: 100%;
        padding: 8px 10px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        background: var(--panel);
        color: var(--ink);
    }
    /* The places open over the list, so the list does not move. */
    .found {
        position: absolute;
        z-index: 6;
        top: calc(100% + 4px);
        right: 0;
        left: 0;
        display: flex;
        flex-direction: column;
        padding: 4px;
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--panel);
        box-shadow: 0 8px 24px rgb(28 27 20 / 0.18);
    }
    .found button {
        display: flex;
        flex-direction: column;
        padding: 6px 10px;
        border: 0;
        border-radius: 5px;
        background: none;
        color: var(--ink);
        text-align: left;
    }
    .found button:hover {
        background: var(--parchment-2);
    }
    small {
        color: var(--ink-soft);
    }
    .note, .count {
        margin: 0;
        color: var(--ink-soft);
    }
    .found .note {
        margin: 4px 8px 6px;
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
