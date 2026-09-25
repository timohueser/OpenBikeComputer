<script lang="ts">
    import { onMount } from "svelte";
    import "../../../../docs/assets/site-header.css";
    import { setupSiteNavigation } from "../../../../docs/assets/site-navigation.js";
    import { appearance, startTheme, toggleTheme } from "../lib/theme.svelte";

    import DeviceChip from "./DeviceChip.svelte";
    import { platform } from "../lib/platform";
    import { available } from "../lib/platform/gating";
    import { router, type Route } from "../lib/router.svelte";
    import { ADVANCED_ROUTE, DEVICE_ROUTE, RIDES_ROUTE } from "../lib/routes";

    let header: HTMLElement;
    onMount(startTheme);
    onMount(() => siteNav ? setupSiteNavigation(header) : undefined);

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

{#snippet brand()}
    <img class="mark" src={`${import.meta.env.BASE_URL}brand/app-icon.svg`} width="32" height="32" alt="" />
    <span class="name">OpenBikeComputer</span>
    <span class="short-name" aria-hidden="true">OBC</span>
{/snippet}

{#snippet appTabs()}
    <nav class="tabs" aria-label="App sections">
        {#each tabs as tab (tab.route)}
            <a href={tab.href} class="tab" class:on={router.route === tab.route}
                aria-current={router.route === tab.route ? "page" : undefined}>
                {tab.label}
            </a>
        {/each}
    </nav>
{/snippet}

<header class="site-head" bind:this={header}>
    <div class="head-inner">
        {#if siteNav}
            <a class="brand" href={siteNav.home} aria-label="OpenBikeComputer home">{@render brand()}</a>
            <button class="site-menu-toggle" type="button" aria-controls="site-navigation" aria-expanded="false">Menu</button>
            <nav class="head-links" id="site-navigation" aria-label="Main navigation">
                <a href={siteNav.docs}>Docs</a>
                <a href={siteNav.blog}>Blog</a>
                <a href="#/" aria-current="page">Maps</a>
                <a href={siteNav.github}>GitHub</a>
            </nav>
        {:else}
            <div class="brand" aria-label="OpenBikeComputer">{@render brand()}</div>
            {@render appTabs()}
        {/if}
        <button class="theme-toggle" type="button" aria-label="Dark mode"
            aria-pressed={appearance.dark} title={appearance.dark ? "Switch to light mode" : "Switch to dark mode"}
            onclick={toggleTheme}>
            <svg viewBox="0 0 24 24" aria-hidden="true">
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
    {#if siteNav && tabbed}
        <div class="app-tools">
            {@render appTabs()}
        </div>
    {/if}
</header>

<style>
    .tabs { display: flex; align-self: stretch; margin-right: auto; overflow-x: auto; }
    .tab { display: flex; align-items: center; min-height: 44px; padding: 0 12px; font-size: 14px; border-block: 2px solid transparent; white-space: nowrap; }
    .tab.on { font-weight: 700; border-bottom-color: var(--amber); }
    .app-tools { display: flex; align-items: center; gap: 12px; width: min(1400px, 100% - 32px); margin: 0 auto; border-top: 1px solid color-mix(in srgb, var(--cream) 25%, transparent); }
    @media (max-width: 1100px) {
        .site-head:has(.tabs) :global(.name) { display: none; }
        .site-head:has(.tabs) :global(.short-name) { display: inline; }
    }
    @media (max-width: 760px) {
        .head-inner > .tabs { order: 3; width: 100%; }
    }
</style>
