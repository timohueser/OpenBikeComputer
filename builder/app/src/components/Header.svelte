<script lang="ts">
    import DeviceChip from "./DeviceChip.svelte";
    import { platform } from "../lib/platform";
    import { available, DESKTOP_ADDS } from "../lib/platform/gating";
    import { router, type Route } from "../lib/router.svelte";
    import { ADVANCED_ROUTE, DESKTOP_ROUTE, DEVICE_ROUTE, RIDES_ROUTE } from "../lib/routes";

    // The header has two shapes, decided by capability rather than host name:
    // an app with more than one place to be gets tabs; a single-page site keeps
    // links. Each tab exists exactly where its route does (`App.svelte` gates
    // the same way), so a tab can never point at a page that falls back home.
    const tabs: Array<{ route: Route; href: string; label: string }> = [
        { route: "home", href: "#/", label: "Map builder" },
        ...(available("styleEditor")
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
            <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
                <circle cx="7" cy="16" r="4.4" fill="none" stroke="var(--forest)" stroke-width="1.8" />
                <circle cx="17" cy="16" r="4.4" fill="none" stroke="var(--forest)" stroke-width="1.8" />
                <path
                    d="M7 16 L10.2 8.5 H15 M15 8.5 L17 16 M7 16 L12.4 16 L10.2 8.5"
                    fill="none"
                    stroke="var(--forest)"
                    stroke-width="1.8"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                />
            </svg>
            <span class="name">OpenBikeComputer</span>
            {#if !tabbed}
                <span class="crumb mono">map builder</span>
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
                    <a href={DESKTOP_ROUTE}>Desktop app</a>
                {/if}
                {#if siteNav}
                    <a href={siteNav.docs}>Docs</a>
                    <a href={siteNav.simulator}>Simulator</a>
                    <a href={siteNav.github}>GitHub</a>
                {/if}
            </nav>
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
        height: var(--head-h);
        background: rgba(236, 232, 207, 0.86);
        backdrop-filter: blur(8px);
        border-bottom: 1px solid var(--line);
    }

    .inner {
        width: min(1400px, 100% - 32px);
        margin: 0 auto;
        height: 100%;
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 18px;
    }

    .brand {
        display: flex;
        align-items: baseline;
        gap: 9px;
        flex: none;
    }

    .brand svg {
        align-self: center;
    }

    .name {
        font-family: var(--serif);
        font-size: 18px;
        font-weight: 600;
        color: var(--ink);
    }

    .crumb {
        font-size: 12px;
        color: var(--ink-faint);
        border: 1px solid var(--parchment-3);
        border-radius: 999px;
        padding: 1px 8px;
    }

    .tabs {
        display: flex;
        align-self: stretch;
        gap: 2px;
        margin-right: auto;
    }

    .tab {
        display: flex;
        align-items: center;
        padding: 0 13px;
        font-size: 13.5px;
        color: var(--ink-faint);
        border-bottom: 2px solid transparent;
        /* keep the text centered despite the indicator border */
        border-top: 2px solid transparent;
    }

    .tab:hover {
        color: var(--ink);
    }

    .tab.on {
        color: var(--ink);
        font-weight: 600;
        border-bottom-color: var(--forest);
    }

    .right {
        display: flex;
        align-items: center;
        gap: 18px;
        min-width: 0;
    }

    .links {
        display: flex;
        gap: 18px;
        font-size: 13.5px;
    }

    .links:empty {
        display: none;
    }

    @media (max-width: 700px) {
        .crumb {
            display: none;
        }

        .links {
            gap: 12px;
        }

        .tab {
            padding: 0 9px;
        }
    }
</style>
