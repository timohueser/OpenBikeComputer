<!-- The search of the Home page: the routes and rides of the rider by name at each key, and places after a pause. A place
     opens the planner there, or the Routes page with the place as its chip. -->
<script lang="ts">
    import { router } from '../../lib/router.svelte';
    import { ROUTES } from '../../lib/routes';
    import { formatDistance } from '../../lib/settings/format';
    import Icon from '../../components/planner/PlannerIcon.svelte';
    import { findPlaces, results, type Box, type FoundPlace, type Item } from '../routes/filter';

    let { items }: { items: Item[] } = $props();

    const ROWS = 4, WAIT_MS = 300;
    let text = $state(''), open = $state(false), places = $state.raw<FoundPlace[] | null>(null), failure = $state('');
    let box = $state<HTMLElement>(), timer: ReturnType<typeof setTimeout> | undefined, controller: AbortController | undefined;

    const found = $derived(text.trim() ? results(items, {}, text) : []);
    const counts = $derived({ route: found.filter(item => item.kind === 'route').length, ride: found.filter(item => item.kind === 'ride').length });
    const day = (at: number) => new Date(at).toLocaleDateString('en-GB', { day: 'numeric', month: 'short' });

    /** The newest route with a line, else the area of the map data. */
    async function view(): Promise<Box> {
        const newest = items.find(item => item.kind === 'route' && item.summary);
        return newest?.summary?.bounds ?? (await import('../../lib/planner/map-data')).config.bounds;
    }

    function input() {
        open = true;
        clearTimeout(timer);
        controller?.abort();
        [places, failure] = [null, ''];
        const sent = text.trim();
        if (!sent) return;
        timer = setTimeout(async () => {
            const current = controller = new AbortController();
            try {
                const next = await findPlaces(sent, await view(), current.signal);
                if (!current.signal.aborted) places = next.slice(0, ROWS);
            } catch (reason) {
                if (!current.signal.aborted) failure = reason instanceof Error ? reason.message : String(reason);
            }
        }, WAIT_MS);
    }

    /** The view of the Routes page that holds the most of the rider's things near the place, with their count. */
    function near({ name, kind, box }: FoundPlace) {
        const chips = { place: { name, kind, box } };
        const routes = results(items.filter(item => item.kind === 'route'), chips, '').length;
        const rides = results(items.filter(item => item.kind === 'ride'), chips, '').length;
        return { count: routes || rides, hash: routes ? ROUTES.routes : `${ROUTES.routes}/ridden`, chips };
    }
</script>

<svelte:window onclick={event => { if (open && !box?.contains(event.target as Node)) open = false; }} />

<div class="search" bind:this={box}>
    <span class="icon"><Icon name="search" size={16} /></span>
    <input type="search" aria-label="Search" placeholder={items.length ? 'Search your routes and rides, or a place' : 'Search a place'}
        bind:value={text} oninput={input} onfocus={() => open = true} onkeydown={event => { if (event.key === 'Escape') open = false; }} />
    {#if open && text.trim()}
        <div class="results" role="region" aria-label="Search results">
            {#if found.length}
                <p class="group">Yours</p>
                <ul>
                    {#each found.slice(0, ROWS) as item (item.id)}
                        <li>
                            <a class="row" href={`${item.kind === 'ride' ? ROUTES.ride : ROUTES.route}/${item.id}`}>
                                <strong>{item.name}</strong>
                                <span class="meta">{[item.kind === 'ride' ? 'Ride' : 'Route', item.kind === 'ride' && day(item.at),
                                    item.summary && formatDistance(item.summary.lengthM)].filter(Boolean).join(' · ')}</span>
                            </a>
                        </li>
                    {/each}
                </ul>
                {#each [['route', 'routes', ROUTES.routes], ['ride', 'rides', `${ROUTES.routes}/ridden`]] as const as [kind, noun, hash] (kind)}
                    {#if counts[kind]}
                        <button type="button" class="more" onclick={() => router.go(hash, { search: { chips: {}, text } })}>
                            Show all {counts[kind].toLocaleString('en')} {counts[kind] === 1 ? noun.slice(0, -1) : noun} in Routes ›
                        </button>
                    {/if}
                {/each}
            {/if}
            <p class="group">Places</p>
            {#if failure}
                <p class="note failure" role="alert">{failure}</p>
            {:else if !places}
                <p class="note">Searching…</p>
            {:else if !places.length}
                <p class="note">No place found.</p>
            {:else}
                <ul>
                    {#each places as place, i (i)}
                        {@const mine = near(place)}
                        <li class="place">
                            <span class="text"><strong>{place.name}</strong><span class="meta">{[place.kind, place.region].filter(Boolean).join(' · ')}</span></span>
                            <span class="buttons">
                                <button type="button" class="button" onclick={() => router.go(`${ROUTES.planner}/new`, { place: { lon: place.lon, lat: place.lat, box: place.box } })}>Plan a route here</button>
                                {#if mine.count}
                                    <button type="button" class="button" onclick={() => router.go(mine.hash, { search: { chips: mine.chips, text: '' } })}>Yours near here ({mine.count})</button>
                                {/if}
                            </span>
                        </li>
                    {/each}
                </ul>
            {/if}
        </div>
    {/if}
</div>

<style>
    .search {
        position: relative;
        width: 100%;
        color: var(--ink);
        text-shadow: none;
    }
    .icon {
        position: absolute;
        top: 14px;
        left: 14px;
        display: flex;
        color: var(--ink-soft);
        pointer-events: none;
    }
    input {
        width: 100%;
        padding: 11px 14px 11px 40px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        background: var(--panel);
        color: var(--ink);
        font: inherit;
        box-shadow: 0 2px 10px rgb(28 27 20 / 0.16);
    }
    .results {
        position: absolute;
        z-index: 3;
        top: calc(100% + 6px);
        right: 0;
        left: 0;
        max-height: min(70vh, 560px);
        padding: 6px 0 10px;
        overflow: auto;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--panel);
        box-shadow: 0 6px 20px rgb(28 27 20 / 0.18);
    }
    ul {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .group {
        margin: 8px 14px 2px;
        color: var(--ink-soft);
        font-size: 12px;
    }
    .row, .place {
        display: flex;
        padding: 6px 14px;
    }
    .row {
        flex-direction: column;
        color: var(--ink);
        text-decoration: none;
    }
    .row:hover {
        background: var(--parchment);
    }
    .place {
        flex-wrap: wrap;
        align-items: center;
        justify-content: space-between;
        gap: 6px 12px;
    }
    .text {
        display: flex;
        flex-direction: column;
    }
    .meta, .note {
        color: var(--ink-soft);
        font-size: 12px;
    }
    .note {
        margin: 4px 14px;
    }
    .failure {
        color: var(--coral);
    }
    .buttons {
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
    }
    .button, .more {
        padding: 4px 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: inherit;
        font-size: 13px;
        white-space: nowrap;
    }
    .more {
        display: block;
        margin: 4px 14px 0;
        border: 0;
        padding: 4px 0;
        background: none;
    }
    .button:hover, .more:hover {
        text-decoration: underline;
    }
</style>
