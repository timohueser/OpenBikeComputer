<script lang="ts">
    import { onMount } from 'svelte';
    import CoverageMap from '../components/coverage/CoverageMap.svelte';
    import SkinStep from '../components/coverage/SkinStep.svelte';
    import Icon from '../components/planner/PlannerIcon.svelte';
    import { CatalogClient } from '../lib/catalog/client';
    import { CoverageStore } from '../lib/coverage/store.svelte';
    import { remembered } from '../lib/device-sync/remembered.svelte';
    import { formatBytes } from '../lib/format';
    import type { NextMap, RouteRecord } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import { planTitle } from '../lib/planner/editor';
    import { isPlan } from '../lib/planner/library';
    import { platform } from '../lib/platform';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';
    import { formatDistance } from '../lib/settings/format';
    import Segmented from '../lib/ui/Segmented.svelte';
    import Window from '../lib/ui/Window.svelte';
    import { deviceArea, sameMap } from './area/area';
    import { routesOutside } from './area/outside';

    let { library, active }: { library: Library; active: boolean } = $props();

    type Tool = 'region' | 'box';
    const tools: { value: Tool; label: string }[] = [{ value: 'region', label: 'Countries and regions' }, { value: 'box', label: 'A box' }];

    let store = $state.raw<CoverageStore | null>(null);
    let failure = $state('');
    /** The error of a Done that did not write. */
    let unsaved = $state('');
    let tool = $state<Tool | 'none'>('region');
    let colours = $state(false);
    /** The stored map that waits: Done writes only a value that differs from it. */
    let stored: NextMap | null = null;
    let routes = $state.raw<RouteRecord[]>([]);
    /** The ids of the routes that wait for the device or are on its card: the warning is about them. */
    let onDevice = $state.raw(new Set<string>());

    const device = $derived(remembered.device);
    const parts = $derived(store?.selection.parts ?? []);
    /** The size of the download, known when each part has its cells. */
    const bytes = $derived(store?.ledger?.isFinal ? store.ledger.totalBytes : null);
    const problem = $derived(store?.indexError ?? store?.resolutionError ?? null);
    const lines = $derived(routes.flatMap(route => {
        const line = route.summary?.line ?? [];
        return line.length ? [Array.from({ length: line.length / 2 }, (_, i): [number, number] => [line[2 * i + 1], line[2 * i]])] : [];
    }));
    const nameOf = (route: RouteRecord) => route.name || (isPlan(route) ? planTitle(route.trip) : 'Route');
    const leaving = $derived.by(() => {
        if (!store || !parts.length || bytes === null) return [];
        const checked = routes.filter(route => onDevice.has(route.id) && route.summary);
        return routesOutside(store, checked.map(route => ({ id: route.id, line: route.summary!.line })))
            .map(outside => ({ ...outside, name: nameOf(checked.find(route => route.id === outside.id)!) }));
    });

    onMount(() => {
        let live = true;
        void (async () => {
            const [{ url, body }, next, all, waiting, sent] = await Promise.all([platform.catalog(), library.state('nextMap'), library.routes(),
                library.state('waiting'), library.state('sent')]);
            if (!live) return;
            const made = new CoverageStore(CatalogClient.fromBody(body, url, { fetchImpl: platform.catalogFetch }), body);
            const start = next ?? deviceArea(remembered.device, made.catalog.regions);
            if (start) made.setArea(start);
            stored = next ?? null;
            const card = remembered.device?.storeId;
            onDevice = new Set([...waiting ?? [], ...Object.entries(sent ?? {}).filter(([, copy]) => copy.device.startsWith(`${card}:`)).map(([id]) => id)]);
            routes = all;
            store = made;
        })().catch(error => { if (live) failure = error instanceof Error ? error.message : String(error); });
        return () => { live = false; };
    });

    /** Writes the map that the device gets next, or none when it is the map on the device or has no part. */
    async function done() {
        if (store) {
            const area = store.area(), known = deviceArea(remembered.device, store.catalog.regions);
            const next = !parts.length || bytes === null || (known && sameMap(area, known)) ? null : { ...area, bytes };
            const same = next && stored ? sameMap(next, stored) && next.bytes === stored.bytes : next === stored;
            try {
                if (!same) await library.setState('nextMap', next);
            } catch (error) {
                unsaved = error instanceof Error ? error.message : String(error);
                return;
            }
        }
        router.go(ROUTES.device, {});
    }
</script>

<main>
    <header>
        <a class="back" href={ROUTES.device}>‹ Device</a>
        <h1>The area of the map</h1>
    </header>
    {#if failure}
        <p class="problem" role="alert">The map catalogue did not load: {failure}</p>
    {:else}
        <div class="cards">
            <div class="map">
                {#if store}<CoverageMap {store} {active} bind:tool rail={false} pale routes={lines} />{/if}
            </div>
            {#if store}
                <section class="side" aria-label="The area">
                    <Segmented label="How to choose the area" options={tools} value={tool === 'box' ? 'box' : 'region'} onChange={value => tool = value} />
                    <p class="hint">{tool === 'box' ? 'Drag on the map to draw a box.' : 'Click the map to add a region.'}</p>
                    {#if parts.length}
                        <ul class="parts">
                            {#each parts as part (part.id)}
                                {@const error = part.kind === 'region' ? store.regionErrors.get(part.regionId) : undefined}
                                <li onmouseenter={() => store!.highlightPartId = part.id} onmouseleave={() => store!.highlightPartId = null}>
                                    <span class="name">{part.name}</span>
                                    <button type="button" class="remove" aria-label={`Remove ${part.name}`} onclick={() => store!.removePart(part.id)}><Icon name="close" size={14} /></button>
                                    {#if error && part.kind === 'region'}
                                        <p class="problem">{error} <button type="button" class="link" onclick={() => store!.retryRegion(part.regionId)}>Try again</button></p>
                                    {/if}
                                </li>
                            {/each}
                        </ul>
                    {/if}
                    {#if leaving.length}
                        <div class="leaving">
                            <h2><span class="dot"></span>{leaving.length === 1 ? '1 route leaves this map' : `${leaving.length} routes leave this map`}</h2>
                            {#each leaving as route (route.id)}
                                <p>“{route.name}” has {formatDistance(route.metres)} {route.region ? `in ${route.region.name}` : 'outside it'}.</p>
                                <div class="answers">
                                    <button type="button" class="button" onclick={() => route.boxes.forEach(box => store!.addBox(box))}>{route.boxes.length === 1 ? 'Add the box around it' : 'Add the boxes around it'}</button>
                                    {#if route.region}
                                        {@const region = route.region}
                                        <button type="button" class="link" onclick={() => store!.addRegion(region.id)}>All of {region.name}</button>
                                    {/if}
                                </div>
                            {/each}
                        </div>
                    {/if}
                    <div class="colours">
                        <span class="label">Colours</span>
                        <span>{store.lightSkin.name} · {store.darkSkin.name}</span>
                        <button type="button" class="button" onclick={() => colours = true}>Change</button>
                    </div>
                    <footer>
                        <div>
                            {#if problem || unsaved}
                                <p class="problem" role="alert">{problem ?? unsaved}</p>
                            {:else if parts.length}
                                <p class="size">{bytes === null ? 'Calculating the size' : `${formatBytes(bytes)} to download`}</p>
                            {/if}
                            {#if device?.map}<p class="second">On the device: {device.map} · {formatBytes(device.bytes.map)}</p>{/if}
                        </div>
                        <button type="button" class="button primary" onclick={done} disabled={!!problem || (parts.length > 0 && bytes === null)}>Done</button>
                    </footer>
                </section>
            {/if}
        </div>
    {/if}
</main>

{#if colours && store}
    <Window title="Colours" onClose={() => colours = false}>
        <SkinStep {store} plain />
    </Window>
{/if}

<style>
    /* A working page: the left edge stands under the logo of the top bar, and the page does not scroll as a whole. */
    main {
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
        gap: 10px 20px;
    }
    .back {
        color: var(--ink-soft);
        text-decoration: none;
    }
    .back:hover {
        color: var(--ink);
    }
    h1 {
        margin: 0;
        font: 700 25px/1.2 var(--sans);
    }
    p {
        margin: 0;
    }
    /* The map is the page; the card is as high as its content. */
    .cards {
        display: grid;
        flex: 1;
        grid-template-columns: 320px 1fr;
        grid-template-rows: minmax(0, 1fr);
        gap: 14px;
        min-height: 0;
    }
    .map {
        grid-column: 2;
        grid-row: 1;
        overflow: hidden;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--parchment-2);
    }
    /* The credit of the map has the letters and the colours of the page. */
    .map :global(.leaflet-control-attribution) {
        padding: 2px 8px;
        border-top-left-radius: 6px;
        color: var(--ink-soft) !important;
        font: 11px/1.5 var(--sans);
    }
    .map :global(.leaflet-control-attribution a) {
        color: inherit;
    }
    .map :global(.leaflet-attribution-flag) {
        display: none !important;
    }
    .side {
        display: flex;
        flex-direction: column;
        grid-column: 1;
        grid-row: 1;
        align-self: start;
        max-height: 100%;
        overflow: auto;
        padding: 16px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--panel);
    }
    .hint, .label, .second {
        color: var(--ink-soft);
    }
    .hint {
        margin: 10px 0 16px;
        font-size: 13.5px;
    }
    .second {
        margin-top: 2px;
        font-size: 13.5px;
    }
    .parts {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .parts li, .leaving, .colours, footer {
        border-top: 1px solid var(--line);
    }
    .parts li {
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
        align-items: center;
        gap: 2px 8px;
        padding: 9px 0;
    }
    .parts .problem {
        grid-column: 1 / -1;
        font-size: 13.5px;
    }
    .remove {
        display: grid;
        place-items: center;
        width: 26px;
        height: 26px;
        margin: -2px -5px -2px 0;
        padding: 0;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
    }
    .remove:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .leaving {
        padding: 14px 0;
    }
    h2 {
        display: flex;
        align-items: center;
        gap: 8px;
        margin: 0 0 4px;
        font: 600 14px/1.4 var(--sans);
    }
    /* The one thing on the page that waits for the rider. */
    .dot {
        width: 6px;
        height: 6px;
        border-radius: 50%;
        background: var(--amber);
    }
    .leaving p {
        color: var(--ink-soft);
        font-size: 13.5px;
    }
    .answers {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 6px 14px;
        margin-top: 10px;
    }
    .answers + p {
        margin-top: 14px;
    }
    .colours {
        display: grid;
        grid-template-columns: auto minmax(0, 1fr) auto;
        align-items: center;
        gap: 12px;
        padding: 12px 0;
    }
    footer {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        padding-top: 14px;
    }
    .size {
        font-weight: 600;
    }
    .button {
        min-height: 34px;
        padding: 5px 14px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        white-space: nowrap;
    }
    .button:hover:not(:disabled) {
        background: var(--parchment-2);
    }
    .primary, .primary:hover:not(:disabled) {
        border-color: var(--rust);
        background: var(--rust);
        color: var(--cream);
    }
    .button:disabled {
        opacity: .6;
    }
    .link {
        padding: 0;
        border: 0;
        background: none;
        color: var(--ink-soft);
        font-size: 13px;
        text-decoration: underline;
        text-underline-offset: 2px;
    }
    .link:hover {
        color: var(--ink);
    }
    .problem {
        color: var(--coral);
    }
    @media (max-width: 760px) {
        main {
            height: auto;
            padding: 12px 16px 16px;
        }
        header .back {
            flex-basis: 100%;
        }
        h1 {
            font-size: 21px;
        }
        .cards {
            grid-template-columns: minmax(0, 1fr);
            grid-template-rows: auto;
        }
        /* The map first, as high as half the screen, then the card. */
        .map, .side {
            grid-column: 1;
            grid-row: auto;
        }
        .map {
            height: max(320px, 52dvh);
        }
        .side {
            max-height: none;
        }
    }
</style>
