<script module lang="ts">
    import type { Component } from 'svelte';
    import type { Route } from './lib/routes';
    import { initCore } from './lib/core/bridge';
    import { Library } from './lib/library/store';

    /** The props that the shell gives each page. `active` is false while a `keep` page is hidden under another page or under the settings. */
    interface PageProps { library: Library; active: boolean }
    interface Page {
        /** The text of its link in the top bar; a page with no label has no link. */
        label?: string;
        /** The page stays mounted, hidden, while another page shows. */
        keep?: boolean;
        load: () => Promise<{ default: Component<PageProps> }>;
    }

    /** A route with no line shows `START`. */
    const START: Route = 'home';
    const PAGES: Partial<Record<Route, Page>> = {
        // The view, the text and the cards show from the library alone; nothing waits for the map catalogue.
        home: { load: () => import('./pages/Home.svelte') },
        // The planner reads the live catalogue as it loads.
        planner: { label: 'Planner', keep: true, load: () => initCore().then(() => import('./components/planner/PlannerApp.svelte')) },
        // The list shows from the library alone; the map and the place search load the map catalogue after it.
        routes: { label: 'Routes', load: () => import('./pages/Routes.svelte') },
        // The page writes GPX and FIT.
        route: { load: () => initCore().then(() => import('./pages/Route.svelte')) },
        // The page reads and writes GPX and FIT.
        ride: { load: () => initCore().then(() => import('./pages/Ride.svelte')) },
        device: { label: 'Device', load: () => import('./pages/Device.svelte') },
        // The page reads the grid of the map cells.
        area: { load: () => initCore().then(() => import('./pages/Area.svelte')) },
    };
</script>

<script lang="ts">
    import { onMount, tick, untrack } from 'svelte';
    import './styles/shell.css';
    import TopBar from './components/TopBar.svelte';
    import Window from './lib/ui/Window.svelte';
    import { ROUTES } from './lib/routes';
    import { router } from './lib/router.svelte';
    import { startTheme } from './lib/theme.svelte';
    import { startCoverJob } from './lib/covers/job';
    import { deviceSync, startDeviceSync } from './lib/device-sync/state.svelte';
    import { deviceName, remembered, rememberedReady } from './lib/device-sync/remembered.svelte';

    const siteBase = import.meta.env.VITE_SITE_BASE || '/';
    // One library for all pages: a listener of `onChange` hears only the writes of its own object.
    const library = new Library();
    const current = $derived(PAGES[router.route] ? router.route : START);
    // A page enters at its first visit. A failed load keeps its message: the next try is a reload of the page.
    let pages = $state.raw<Partial<Record<Route, { component?: Component<PageProps>; error?: string }>>>({});
    let coversStarted = false;
    const device = $derived(remembered.device && {
        name: deviceName(remembered.device.serial),
        connected: rememberedReady(),
        // The sync can run again only with the device on the cable.
        sync: deviceSync.running ? 'running' as const : rememberedReady() && deviceSync.problem ? 'problem' as const : null,
    });
    const links = $derived(Object.entries(PAGES).flatMap(([route, page]) =>
        page.label ? [{ href: ROUTES[route as Route], label: page.label, current: route === current || ROUTES[current].startsWith(`${ROUTES[route as Route]}/`) }] : []));

    $effect(() => {
        const route = current;
        if (untrack(() => pages[route])) return;
        pages = { ...untrack(() => pages), [route]: {} };
        PAGES[route]!.load().then(async ({ default: component }) => {
            pages = { ...pages, [route]: { component } };
            if (coversStarted) return;
            coversStarted = true;
            await tick();
            startCoverJob(library);
        }, (error: unknown) => {
            pages = { ...pages, [route]: { error: error instanceof Error ? error.message : String(error) } };
        });
    });

    // An open modal window makes the rest of the document inert, also when it is in a hidden page.
    $effect(() => {
        void [router.route, router.id];
        untrack(() => document.querySelectorAll('dialog[open]').forEach(dialog => (dialog as HTMLDialogElement).close()));
    });

    onMount(startTheme);
    onMount(() => startDeviceSync(library));
</script>

<div class="shell">
    <TopBar {links} {device} onsync={() => deviceSync.run()} settings={router.settings} />
    <div class="pages">
        {#each Object.entries(pages) as [route, page] (route)}
            {#if route === current || (PAGES[route as Route]!.keep && page.component)}
                <!-- No `display` rule here: it would win over `hidden`. -->
                <div class="page" hidden={route !== current}>
                    {#if page.error}
                        <p class="failure" role="alert">{page.error}</p>
                    {:else if page.component}
                        <page.component {library} active={route === current && !router.settings} />
                    {/if}
                </div>
            {/if}
        {/each}
    </div>
    {#if router.settings}
        {#await import('./pages/Settings.svelte') then { default: Settings }}
            <Settings {library} />
        {:catch error}
            <Window title="Settings" large onClose={() => router.closeSettings()}>
                <p role="alert">{error instanceof Error ? error.message : String(error)}</p>
            </Window>
        {/await}
    {/if}
    <footer>
        <a href={siteBase}>OpenBikeComputer</a>
        <a href={`${siteBase}docs/`}>Docs</a>
        <a href={`${siteBase}blog/`}>Blog</a>
        <a href={`${siteBase}builder/`}>Map builder</a>
        <a href={`${siteBase}docs/impressum/`}>Impressum</a>
        <a href={`${siteBase}docs/datenschutz/`}>Datenschutz</a>
    </footer>
</div>

<style>
    .shell {
        display: flex;
        flex-direction: column;
        height: 100dvh;
    }
    /* The narrow breakpoint of the planner: the document scrolls, so a phone browser can hide its toolbar. */
    @media (max-width: 760px) {
        .shell { height: auto; min-height: 100dvh; }
        .pages { flex: 1 0 auto; }
        .page { height: auto; overflow: visible; }
    }
    .pages {
        flex: 1;
        min-height: 0;
    }
    .page {
        height: 100%;
        overflow: auto;
    }
    .failure {
        margin: 32px 16px;
    }
    footer {
        display: flex;
        flex: none;
        flex-wrap: wrap;
        gap: 4px 16px;
        padding: 6px 16px;
        border-top: 1px solid var(--line);
        background: var(--panel);
        font-size: 11px;
    }
    footer a {
        color: var(--ink-soft);
        text-decoration: none;
    }
    footer a:hover {
        color: var(--ink);
        text-decoration: underline;
    }
</style>
