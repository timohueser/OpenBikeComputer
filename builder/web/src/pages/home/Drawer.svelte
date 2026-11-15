<!-- The drawer under the home view: the newest routes or the newest rides as cards, or the previews of the features on a
     first visit. -->
<script lang="ts">
    import type { RideRecord, RouteRecord } from '../../lib/library/records';
    import type { Library } from '../../lib/library/store';
    import Segmented from '../../lib/ui/Segmented.svelte';
    import { ROUTES } from '../../lib/routes';
    import { rideItem, routeItem } from '../routes/filter';
    import ItemCard from './ItemCard.svelte';

    let { library, routes, rides, credits = $bindable([]) }: {
        library: Library;
        routes: RouteRecord[];
        rides: RideRecord[];
        /** The credits of the pictures of the shown cards. */
        credits?: string[];
    } = $props();

    type Kind = 'routes' | 'rides';
    const CARDS = 6;
    let chosen = $state<Kind>('routes');
    const kind: Kind = $derived(rides.length && (chosen === 'rides' || !routes.length) ? 'rides' : 'routes');
    const items = $derived((kind === 'rides' ? rides.map(rideItem) : routes.map(route => routeItem(route))).sort((a, b) => b.at - a.at).slice(0, CARDS));
    const count = $derived(kind === 'rides' ? rides.length : routes.length);
    let byCard = $state<Record<string, string[]>>({});
    $effect(() => { credits = [...new Set(items.flatMap(item => byCard[item.id] ?? []))]; });
    const PREVIEWS = [
        { name: 'Plan a route', text: 'A day ride or a trip of many days, with nights and stops.' },
        { name: 'Look around before you go', text: 'TrailView shows the land along your route in 3-D.' },
        { name: 'Keep routes and rides', text: 'Search them by place, year or activity.' },
        { name: 'Take it with you', text: 'GPX and FIT for each device, or the OpenBikeComputer.' },
    ];
</script>

<section class="drawer" aria-labelledby="home-drawer">
    {#if count}
        <header>
            <div class="title">
                <h2 id="home-drawer" class:unseen={routes.length && rides.length}>{kind === 'rides' ? 'Your rides' : 'Your routes'}</h2>
                {#if routes.length && rides.length}
                    <Segmented label="Routes or rides" value={kind} onChange={next => chosen = next} compact
                        options={[{ value: 'routes', label: 'Routes' }, { value: 'rides', label: 'Rides' }]} />
                {/if}
            </div>
            <a class="all" href={kind === 'rides' ? `${ROUTES.routes}/ridden` : ROUTES.routes}>All {count.toLocaleString('en')} in Routes ›</a>
        </header>
        <ul class="cards items" style:--columns={Math.max(items.length, 4)}>
            {#each items as item (item.id)}
                <ItemCard {library} {item} oncredits={list => { if (JSON.stringify(list) !== JSON.stringify(byCard[item.id] ?? [])) byCard[item.id] = list; }} />
            {/each}
        </ul>
    {:else}
        <header>
            <h2 id="home-drawer">What you can do here</h2>
        </header>
        <ul class="cards">
            {#each PREVIEWS as preview, i (preview.name)}
                <li class="preview">
                    <svg viewBox="0 0 240 92" aria-hidden="true">
                        {#if i === 0}
                            <path class="text-line" d="M14 20 H62 M14 34 H62 M14 48 H50 M14 62 H56" />
                            <path class="route" d="M86 72 C120 74 130 48 160 44 S200 28 222 18" />
                            <circle cx="86" cy="72" r="5" /><circle cx="222" cy="18" r="5" />
                        {:else if i === 1}
                            <path class="hill-far" d="M0 60 L50 34 L100 52 L150 26 L200 46 L240 34 V92 H0Z" />
                            <path class="hill-near" d="M0 74 C60 60 120 70 240 58 V92 H0Z" />
                            <path class="route" d="M10 84 C70 72 140 66 230 50" />
                        {:else if i === 2}
                            {#each [24, 92, 160] as x (x)}
                                <rect class="tile" x={x} y="14" width="56" height="64" rx="6" />
                                <path class="ride" d={`M${x + 10} 62 C${x + 22} 60 ${x + 26} 34 ${x + 46} 28`} />
                            {/each}
                        {:else}
                            <path class="dash" d="M20 46 H82 M158 46 H220" />
                            <!-- The housing of pages/device/Drawing.svelte, in its units: a body 308 × 470. -->
                            <g transform="translate(96 10) scale(0.155)">
                                <rect class="rim" x="-6" y="-6" width="320" height="482" rx="48" />
                                <rect class="body" width="308" height="470" rx="42" />
                                {#each [[-13, 158], [-13, 246], [302, 158], [302, 246]] as [x, y] (`${x}:${y}`)}
                                    <rect class="key" {x} {y} width="19" height="66" rx="2" />
                                {/each}
                                <rect class="bezel" x="18" y="16" width="272" height="352" rx="26" />
                                <rect class="screen" x="34" y="32" width="240" height="320" rx="10" />
                                <path class="route on-screen" d="M80 310 C110 220 210 230 230 70" />
                            </g>
                        {/if}
                    </svg>
                    <strong>{preview.name}</strong>
                    <p class="meta">{preview.text}</p>
                </li>
            {/each}
        </ul>
    {/if}
</section>

<style>
    .drawer {
        display: flex;
        flex: none;
        flex-direction: column;
        gap: 10px;
        padding: 14px 40px 18px;
        border-top: 1px solid var(--line);
        background: var(--panel);
    }
    header {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        justify-content: space-between;
        gap: 4px 16px;
        min-height: 30px;
    }
    h2 {
        margin: 0;
        font: 700 15px/1.3 var(--sans);
    }
    .title {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 8px 14px;
    }
    /* The switch names the kind, so the heading is for a screen reader only. */
    .unseen {
        position: absolute;
        width: 1px;
        height: 1px;
        overflow: hidden;
        clip-path: inset(50%);
    }
    .meta {
        margin: 0;
        color: var(--ink-soft);
        font-size: 12px;
    }
    .all {
        color: var(--ink-soft);
        text-decoration: none;
        white-space: nowrap;
    }
    .all:hover {
        color: var(--ink);
        text-decoration: underline;
    }
    .cards {
        display: grid;
        grid-auto-columns: minmax(190px, 1fr);
        grid-auto-flow: column;
        gap: 12px;
        margin: 0;
        padding: 0 0 4px;
        overflow-x: auto;
        list-style: none;
    }
    /* The cards share the row; fewer than four keep the width of one of four. */
    .items {
        grid-auto-columns: minmax(190px, calc((100% - (var(--columns) - 1) * 12px) / var(--columns)));
    }
    /* A preview is a picture with its text, with no frame: it is not a card to open. */
    .preview {
        display: flex;
        flex-direction: column;
        gap: 2px;
        min-width: 0;
    }
    .preview svg {
        height: 92px;
        margin-bottom: 6px;
        border-radius: 8px;
        background: var(--parchment-2);
    }
    .preview circle {
        fill: var(--ink);
    }
    .route, .ride, .text-line, .dash {
        fill: none;
        stroke-linecap: round;
        stroke-width: 3;
    }
    .route {
        stroke: var(--route);
    }
    .ride {
        stroke: var(--ride);
    }
    .text-line {
        stroke: var(--parchment-3);
        stroke-width: 4;
    }
    .dash {
        stroke: var(--ink-soft);
        stroke-dasharray: 4 5;
        stroke-width: 2;
    }
    .hill-far {
        fill: var(--parchment-3);
    }
    .hill-near {
        fill: var(--water);
        opacity: .45;
    }
    .tile {
        fill: var(--panel);
        stroke: var(--line-strong);
    }
    /* The colours of the housing, the same in both looks, as on the Device page. */
    .rim {
        fill: #8be3bc;
    }
    .body {
        fill: #2f6350;
    }
    .key {
        fill: #2b2f36;
    }
    .bezel {
        fill: #101317;
    }
    .screen {
        fill: #f4f2eb;
    }
    .on-screen {
        stroke: #cc2a93;
        stroke-width: 16;
    }
    @media (max-width: 760px) {
        .drawer {
            padding: 12px 16px 16px;
        }
    }
</style>
