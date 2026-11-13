<!-- The Home page: the view of the day with the personal text, and a drawer of the newest routes and rides. -->
<script lang="ts">
    import { onMount } from 'svelte';
    import type { RideRecord, RouteRecord } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import Drawer from './home/Drawer.svelte';
    import Personal from './home/Personal.svelte';
    import { homeText, type HomeText } from './home/personal';
    import View from './home/View.svelte';
    import { homeNow } from './home/views';

    let { library }: { library: Library; active: boolean } = $props();

    const READ_AFTER_MS = 200;
    const now = homeNow();
    let routes = $state.raw<RouteRecord[]>(), rides = $state.raw<RideRecord[]>(), text = $state.raw<HomeText>();
    let shown = $state(false), failure = $state('');

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
        <View {now} bind:shown />
        {#if text}<Personal {library} {text} />{/if}
    </section>
    {#if failure}
        <p class="failure" role="alert">{failure}</p>
    {:else if routes && rides}
        <Drawer {library} {routes} {rides} />
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
        min-height: 360px;
        padding: 96px 40px 24px;
        overflow: hidden;
        color: var(--ink);
    }
    .hero.shown {
        background: #3a4640;
        color: #fff;
        text-shadow: 0 1px 12px rgb(0 0 0 / 0.55);
    }
    .hero > :global(.personal) {
        position: relative;
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
            padding: 32px 16px 16px;
            gap: 24px;
        }
    }
</style>
