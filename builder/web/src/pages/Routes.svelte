<!-- The Routes page: the planned routes in a list with a search and chips, on a map, and the selected one in a card. -->
<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import { SvelteSet } from 'svelte/reactivity';
    import Card from '../lib/ui/Card.svelte';
    import Switch from '../lib/ui/Switch.svelte';
    import Cover from '../lib/covers/Cover.svelte';
    import { initCore } from '../lib/core/bridge';
    import type { LibraryState, RouteRecord } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import { ridingProfiles } from '../lib/planner/riding-profiles';
    import { ROUTES } from '../lib/routes';
    import { formatDistance, formatHeight } from '../lib/settings/format';
    import Chips from './routes/Chips.svelte';
    import { readSearch, results, routeItem, type Box, type Chips as ChipValues, type Naming } from './routes/filter';
    import SelectedCard from './routes/SelectedCard.svelte';
    import { deviceState, type DeviceState } from './routes/state';

    let { library }: { library: Library; active: boolean } = $props();

    /** A page that sends the rider here can give the search to start with. */
    const start = history.state?.search as { chips?: ChipValues; text?: string } | undefined;
    const READ_AFTER_MS = 200, KEEP_AFTER_MS = 300;
    const BADGES: Record<DeviceState, string> = { waiting: 'Waits', sent: 'On the device', changed: 'Changed' };

    let records = $state.raw<RouteRecord[]>(), waiting = $state.raw<LibraryState['waiting']>(), sent = $state.raw<LibraryState['sent']>();
    let naming = $state.raw<Naming>(), failure = $state('');
    let chips = $state<ChipValues>(start?.chips ?? {}), text = $state(start?.text ?? '');
    let onlyView = $state(false), view = $state.raw<Box>(), selected = $state<string>(), fit = $state(0), adding = $state(false);
    /** The rows in view or near it: only these mount their picture. */
    const near = new SvelteSet<string>();

    const items = $derived(records?.map(record => routeItem(record, naming)) ?? []);
    const shown = $derived(results(items, chips, text, onlyView ? view : undefined));
    const states = $derived(new Map(records?.map(record => [record.id, deviceState(record, waiting, sent)])));
    const chosen = $derived(shown.find(item => item.id === selected));
    const routes = (count: number) => `${count.toLocaleString('en')} route${count === 1 ? '' : 's'}`;
    const listed = $derived(records !== undefined);

    // The map fits the results when the list first shows and after a chip change, unless it keeps its view. A reload of
    // the library after a write does not move it.
    $effect(() => {
        void JSON.stringify(chips);
        if (listed && !untrack(() => onlyView)) untrack(() => fit++);
    });

    // Back from another page brings the search again. WebKit refuses many history writes in a short time, so the page
    // writes once after the last key, and at once before a link changes the entry.
    let keep: (() => void) | undefined;
    $effect(() => {
        const search = $state.snapshot({ chips, text });
        keep = () => { keep = undefined; history.replaceState({ ...history.state, search }, ''); };
        const timer = setTimeout(keep, KEEP_AFTER_MS);
        return () => clearTimeout(timer);
    });

    async function load() {
        try {
            [records, waiting, sent] = await Promise.all([library.routes(), library.state('waiting'), library.state('sent')]);
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
            if (store !== 'routes' && !(store === 'state' && (key === 'waiting' || key === 'sent'))) return;
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

    const addFiles = () => initCore().then(() => import('./AddFiles.svelte')).then(module => module.default);
</script>

<svelte:window onclickcapture={event => { if ((event.target as Element).closest?.('a[href]')) keep?.(); }} />

<div class="routes-page">
    <header>
        <h1>Routes</h1>
        <nav class="buttons" aria-label="Routes">
            <a class="button primary" href={`${ROUTES.planner}/new`}>New route</a>
            <button type="button" class="button" onclick={() => adding = true}>Add files</button>
        </nav>
    </header>
    {#if failure}<p class="failure" role="alert">{failure}</p>{/if}

    <div class="cards">
        <div class="list">
            <Card>
                {#if records === undefined}
                    <p class="note">Loading…</p>
                {:else if !records.length}
                    <p class="note">No routes yet. Plan one, or add GPX and FIT files.</p>
                {:else}
                    <input class="search" type="search" aria-label="Search your routes" placeholder="Search your routes, or a place"
                        bind:value={text} onkeydown={event => { if (event.key === 'Enter' && text.trim()) void understand(); }} />
                    <Chips bind:chips {view} />
                    <p class="count">{shown.length === items.length ? routes(items.length) : `${shown.length.toLocaleString('en')} of ${routes(items.length)}`}</p>
                    <ul>
                        {#each shown as item (item.id)}
                            {@const state = states.get(item.id)}
                            <li data-id={item.id} use:watch>
                                <button type="button" class="row" aria-current={item.id === selected} onclick={() => selected = item.id}>
                                    <span class="thumb">{#if item.summary && near.has(item.id)}<Cover {library} id={item.id} />{/if}</span>
                                    <span class="text">
                                        <strong>{item.name}</strong>
                                        {#if item.summary}
                                            {@const summary = item.summary}
                                            <span class="meta">{[ridingProfiles[summary.bike].label, (summary.days ?? 1) > 1 && `${summary.days} days`,
                                                formatDistance(summary.lengthM), `${formatHeight(summary.climbM)} up`].filter(Boolean).join(' · ')}</span>
                                        {/if}
                                    </span>
                                    {#if state}<span class="badge {state}">{BADGES[state]}</span>{/if}
                                </button>
                            </li>
                        {:else}
                            <li class="note">No route matches. Remove a chip or change the text.</li>
                        {/each}
                    </ul>
                    <div class="switch"><Switch bind:checked={onlyView} label="Only in this map view" /></div>
                {/if}
            </Card>
        </div>
        <div class="map">
            {#await import('./routes/RoutesMap.svelte') then { default: RoutesMap }}
                <RoutesMap {items} {shown} {selected} {fit} onselect={id => selected = id} onview={next => view = next} />
            {:catch error}
                <p class="note">{error instanceof Error ? error.message : String(error)}</p>
            {/await}
            {#if chosen}
                <div class="selected"><SelectedCard {library} item={chosen} device={states.get(chosen.id)} /></div>
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
        grid-template-columns: 360px 1fr;
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
        font-size: 12px;
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
        grid-template-columns: 64px minmax(0, 1fr) auto;
        align-items: center;
        gap: 10px;
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
        height: 44px;
        overflow: hidden;
        border-radius: 5px;
        background: var(--parchment);
    }
    .text {
        display: flex;
        flex-direction: column;
        min-width: 0;
    }
    .meta {
        color: var(--ink-soft);
        font-size: 12px;
    }
    .badge {
        padding: 1px 6px;
        border-radius: 4px;
        background: var(--parchment-2);
        color: var(--ink-soft);
        font-size: 11px;
        white-space: nowrap;
    }
    .badge.waiting {
        background: var(--amber);
        color: var(--on-amber);
    }
    .switch {
        padding-top: 6px;
        border-top: 1px solid var(--line);
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
        .map {
            height: 70vh;
        }
    }
</style>
