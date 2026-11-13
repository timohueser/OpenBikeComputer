<!-- The drawer under the home view: the newest routes and rides as cards, or the previews of the features on a first
     visit. -->
<script lang="ts">
    import type { RideRecord, RouteRecord } from '../../lib/library/records';
    import type { Library } from '../../lib/library/store';
    import { ROUTES } from '../../lib/routes';
    import { rideItem, routeItem } from '../routes/filter';
    import ItemCard from './ItemCard.svelte';

    let { library, routes, rides }: { library: Library; routes: RouteRecord[]; rides: RideRecord[] } = $props();

    const CARDS = 6;
    const items = $derived([...routes.map(route => routeItem(route)), ...rides.map(rideItem)].sort((a, b) => b.at - a.at).slice(0, CARDS));
    const count = $derived(routes.length + rides.length);
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
            <h2 id="home-drawer">Your routes and rides</h2>
            <a class="button" href={ROUTES.routes}>All {count.toLocaleString('en')} in Routes ›</a>
        </header>
        <ul class="cards items">
            {#each items as item (item.id)}<ItemCard {library} {item} />{/each}
        </ul>
    {:else}
        <header>
            <h2 id="home-drawer">What you can do here</h2>
            <p class="note">Your routes and rides take this place when you have some.</p>
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
                            <rect class="device" x="96" y="8" width="48" height="76" rx="6" />
                            <rect class="screen" x="102" y="16" width="36" height="52" />
                            <path class="route" d="M110 20 C112 40 132 44 130 64" />
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
        padding: 12px 20px 16px;
        border-top: 1px solid var(--line);
        background: var(--panel);
    }
    header {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        justify-content: space-between;
        gap: 4px 16px;
    }
    h2 {
        margin: 0;
        font: 700 15px/1.3 var(--sans);
    }
    header:has(.note) {
        justify-content: flex-start;
    }
    .note, .meta {
        margin: 0;
        color: var(--ink-soft);
        font-size: 12px;
    }
    .button {
        padding: 4px 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        color: var(--ink);
        text-decoration: none;
        white-space: nowrap;
    }
    .button:hover {
        background: var(--parchment-2);
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
    /* A card keeps the width of one of six, also when there are fewer. */
    .items {
        grid-auto-columns: minmax(190px, calc((100% - 60px) / 6));
    }
    .preview {
        display: flex;
        flex-direction: column;
        gap: 2px;
        min-width: 0;
        padding: 0 10px 10px;
        overflow: hidden;
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--parchment);
    }
    .preview svg {
        height: 92px;
        margin: 0 -10px 6px;
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
    .device {
        fill: var(--ink);
    }
    .screen {
        fill: var(--parchment-2);
    }
</style>
