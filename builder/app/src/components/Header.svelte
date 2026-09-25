<script lang="ts">
    import { onMount } from "svelte";
    import { appearance, startTheme, toggleTheme } from "../lib/theme.svelte";

    import DeviceChip from "./DeviceChip.svelte";
    import { platform } from "../lib/platform";
    import { available, DESKTOP_ADDS } from "../lib/platform/gating";
    import { router, type Route } from "../lib/router.svelte";
    import { ADVANCED_ROUTE, DESKTOP_ROUTE, DEVICE_ROUTE, RIDES_ROUTE } from "../lib/routes";

    onMount(startTheme);

    // The header has two shapes, decided by capability rather than host name:
    // an app with more than one place to be gets tabs; a single-page site keeps
    // links. Each tab exists exactly where its route does (`App.svelte` gates
    // the same way), so a tab can never point at a page that falls back home.
    const tabs: Array<{ route: Route; href: string; label: string }> = [
        { route: "home", href: "#/", label: "Maps" },
        ...(platform.styleEditor
            ? [{ route: "advanced" as const, href: ADVANCED_ROUTE, label: "Style editor" }]
            : []),
        ...(available("deviceDashboard")
            ? [{ route: "device" as const, href: DEVICE_ROUTE, label: "Device" }]
            : []),
        ...(available("rideLibrary")
            ? [{ route: "rides" as const, href: RIDES_ROUTE, label: "Ride library" }]
            : []),
    ];
    const tabbed = tabs.length > 1;

    // Links out of the app, present only where there is a site around it (the
    // desktop app has none — `platform.siteNav` is absent there, and so are
    // these). Orthogonal to the tabs: the dev server shows both.
    const siteNav = platform.siteNav;
</script>

<header>
    <div class="inner">
        <div class="brand">
            <span class="name">OpenBikeComputer</span>
            <span class="short-name" aria-label="OpenBikeComputer">OBC</span>
            {#if !tabbed}
                <span class="crumb mono">maps</span>
            {/if}
        </div>

        {#if tabbed}
            <nav class="tabs" aria-label="App sections">
                {#each tabs as tab (tab.route)}
                    <a href={tab.href} class="tab" class:on={router.route === tab.route}
                        aria-current={router.route === tab.route ? "page" : undefined}>
                        {tab.label}
                    </a>
                {/each}
            </nav>
        {/if}

        <div class="right">
            <nav class="links">
                <!-- Nav chrome is the one place a missing feature is better left
                     out than shown dead: there is no intent behind a link, so a
                     greyed one explains nothing anyone was asking. -->
                {#if DESKTOP_ADDS.length}
                    <a class="desktop-link" href={DESKTOP_ROUTE}>Desktop app</a>
                {/if}
                {#if siteNav}
                    <a href={siteNav.docs}>Docs</a>
                    <a class="simulator-link" href={siteNav.simulator}>Simulator</a>
                    <a class="github-link" href={siteNav.github}>GitHub</a>
                {/if}
            </nav>
            <button class="theme-toggle" type="button" aria-label="Dark mode"
                aria-pressed={appearance.dark} title={appearance.dark ? "Switch to light mode" : "Switch to dark mode"}
                onclick={toggleTheme}>
                <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
                    {#if appearance.dark}
                        <circle cx="12" cy="12" r="4" />
                        <path d="M12 2v2m0 16v2M2 12h2m16 0h2M5 5l1.5 1.5m11 11L19 19M5 19l1.5-1.5m11-11L19 5" />
                    {:else}
                        <path d="M20.5 13.2A8.6 8.6 0 0 1 10.8 3.5 8.6 8.6 0 1 0 20.5 13.2Z" />
                    {/if}
                </svg>
            </button>
            {#if available("deviceDashboard")}
                <DeviceChip />
            {/if}
        </div>
    </div>
</header>

<style>
    header {
        position: sticky;
        top: 0;
        z-index: 1100;
        flex: none;
        min-height: var(--head-h);
        background: var(--rust);
        color: var(--cream);
    }
    .inner {
        width: min(1400px, 100% - 32px);
        margin: 0 auto;
        min-height: var(--head-h);
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 18px;
    }
    .brand { display: flex; align-items: baseline; gap: 12px; flex: none; }
    .name, .short-name { font-family: var(--mono); font-size: 18px; font-weight: 700; }
    .short-name { display: none; }
    .crumb { font-size: 13px; }
    .tabs { display: flex; align-self: stretch; margin-right: auto; }
    .tab {
        display: flex;
        align-items: center;
        padding: 0 12px;
        font-size: 14px;
        border-block: 2px solid transparent;
        white-space: nowrap;
    }
    a { color: var(--cream); }
    a:hover { color: var(--cream); }
    .tab.on { font-weight: 700; border-bottom-color: var(--amber); }
    .right, .links { display: flex; align-items: center; gap: 18px; }
    .links { font-size: 14px; }
    .links:empty { display: none; }
    .theme-toggle {
        display: grid;
        place-items: center;
        width: 40px;
        height: 40px;
        flex: none;
        padding: 0;
        border: 1px solid color-mix(in srgb, var(--cream) 40%, transparent);
        border-radius: 6px;
        background: transparent;
        color: var(--cream);
    }
    .theme-toggle:hover { background: #ffffff12; }
    :focus-visible { outline-color: var(--cream); }
    @media (max-width: 1100px) {
        .name { display: none; }
        .short-name { display: inline; }
        .right, .links { gap: 12px; }
        .github-link { display: none; }
    }
    @media (max-width: 700px) {
        .inner { flex-wrap: wrap; gap: 0 12px; }
        .tabs { order: 3; width: 100%; overflow-x: auto; }
        .tab { min-height: 44px; padding: 0 10px; }
    }
    @media (max-width: 480px) {
        .simulator-link, .desktop-link { display: none; }
    }
</style>
