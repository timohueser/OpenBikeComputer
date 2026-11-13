<!-- The Home page: the view of the day with the search and the personal text, and a drawer of the newest routes or rides. -->
<script lang="ts">
    import { onMount } from 'svelte';
    import type { RideRecord, RouteRecord } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import Drawer from './home/Drawer.svelte';
    import Personal from './home/Personal.svelte';
    import { homeText, type HomeText } from './home/personal';
    import Search from './home/Search.svelte';
    import View from './home/View.svelte';
    import { homeNow } from './home/views';
    import { rideItem, routeItem } from './routes/filter';

    let { library }: { library: Library; active: boolean } = $props();

    const READ_AFTER_MS = 200;
    const now = homeNow();
    let routes = $state.raw<RouteRecord[]>(), rides = $state.raw<RideRecord[]>(), text = $state.raw<HomeText>();
    let shown = $state(false), failure = $state(''), credits = $state<string[]>([]);
    const items = $derived([...routes?.map(route => routeItem(route)) ?? [], ...rides?.map(rideItem) ?? []].sort((a, b) => b.at - a.at));

    async function load() {
        try {
            const [nextRoutes, nextRides, active] = await Promise.all([library.routes(), library.rides(), library.state('active')]);
            [routes, rides, text] = [nextRoutes, nextRides, homeText(nextRoutes, nextRides, active, now)];
        } catch (reason) {
            failure = reason instanceof Error ? reason.message : String(reason);
        }
    }

    // A cover reaches only its card.
    onMount(() => {
        void load();
        let timer: ReturnType<typeof setTimeout> | undefined;
        const stop = library.onChange(store => {
            if (store !== 'routes' && store !== 'rides' && store !== 'state') return;
            clearTimeout(timer);
            timer = setTimeout(load, READ_AFTER_MS);
        });
        return () => { stop(); clearTimeout(timer); };
    });
</script>

<div class="home">
    <section class="hero" class:shown aria-label="Home">
        <View {now} cards={credits} bind:shown />
        <div class="search"><Search {items} /></div>
        {#if text}<Personal {library} {text} />{/if}
    </section>
    {#if failure}
        <p class="failure" role="alert">{failure}</p>
    {:else if routes && rides}
        <Drawer {library} {routes} {rides} bind:credits />
    {/if}
</div>

<style>
    .home {
        display: flex;
        flex-direction: column;
        height: 100%;
    }
    /* The image fills a box that the layout sizes, so nothing moves while it loads. */
    .hero {
        position: relative;
        display: flex;
        flex: 1;
        flex-direction: column;
        gap: 40px;
        min-height: 360px;
        padding: 32px 40px;
        background: var(--parchment-2);
        color: var(--ink);
    }
    .hero.shown {
        background: #3a4640;
        color: #fff;
        text-shadow: 0 1px 2px rgb(0 0 0 / 0.45);
    }
    .hero > :global(.personal) {
        position: relative;
    }
    /* The search and the text are one column, at one left edge. */
    .search {
        position: relative;
        z-index: 2;
        width: min(520px, 100%);
    }
    .failure {
        margin: 16px 20px;
        color: var(--coral);
    }
    @media (max-width: 760px) {
        .home {
            height: auto;
        }
        .hero {
            min-height: 78vh;
            padding: 16px;
            gap: 24px;
        }
    }
</style>
