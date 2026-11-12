<script module lang="ts">
    import type { Component } from 'svelte';
    import type { Route } from './lib/routes';
    import { initCore } from './lib/core/bridge';
    import { Library } from './lib/library/store';

    /** The props that the shell gives each page. `active` is false while a `keep` page is hidden under another page. */
    interface PageProps { library: Library; active: boolean }
    interface Page {
        /** The text of its link in the top bar; a page with no label has no link. */
        label?: string;
        /** The page stays mounted, hidden, while another page shows. */
        keep?: boolean;
        load: () => Promise<{ default: Component<PageProps> }>;
    }

    /** A route with no line shows `START`. */
    const START: Route = 'planner';
    const PAGES: Partial<Record<Route, Page>> = {
        // The planner reads the live catalogue as it loads.
        planner: { label: 'Planner', keep: true, load: () => initCore().then(() => import('./components/planner/PlannerApp.svelte')) },
    };
</script>

<script lang="ts">
    import { onMount, tick, untrack } from 'svelte';
    import './styles/shell.css';
    import TopBar from './components/TopBar.svelte';
    import { ROUTES } from './lib/routes';
    import { router } from './lib/router.svelte';
    import { startTheme } from './lib/theme.svelte';
    import { startCoverJob } from './lib/covers/job';

    const siteBase = import.meta.env.VITE_SITE_BASE || '/';
    // One library for all pages: a listener of `onChange` hears only the writes of its own object.
    const library = new Library();
    const current = $derived(PAGES[router.route] ? router.route : START);
    // A page enters at its first visit. A failed load keeps its message: the next try is a reload of the page.
    let pages = $state.raw<Partial<Record<Route, { component?: Component<PageProps>; error?: string }>>>({});
    let coversStarted = false;
    const links = $derived(Object.entries(PAGES).flatMap(([route, page]) =>
        page.label ? [{ href: ROUTES[route as Route], label: page.label, current: route === current }] : []));

    $effect(() => {
        const route = current;
        if (untrack(() => pages[route])) return;
        pages = { ...untrack(() => pages), [route]: {} };
        PAGES[route]!.load().then(async ({ default: component }) => {
            pages = { ...pages, [route]: { component } };
            await tick();
            if (!coversStarted) startCoverJob(library);
            coversStarted = true;
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
</script>

<div class="shell">
    <TopBar {links} />
    <div class="pages">
        {#each Object.entries(pages) as [route, page] (route)}
            {#if route === current || (PAGES[route as Route]!.keep && page.component)}
                <!-- No `display` rule here: it would win over `hidden`. -->
                <div class="page" hidden={route !== current}>
                    {#if page.error}
                        <p class="failure" role="alert">{page.error}</p>
                    {:else if page.component}
                        <page.component {library} active={route === current} />
                    {/if}
                </div>
            {/if}
        {/each}
    </div>
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
