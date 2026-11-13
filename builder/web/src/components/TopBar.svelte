<script lang="ts">
    import Icon from './planner/PlannerIcon.svelte';
    import { ROUTES } from '../lib/routes';

    let { links, device, onsync, settings }: {
        links: { href: string; label: string; current: boolean }[];
        /** The remembered device; null before a first connection. `sync` is `problem` after a problem of the last sync. */
        device: { name: string; connected: boolean; sync: 'running' | 'problem' | null } | null;
        onsync: () => void;
        settings: boolean;
    } = $props();
    // A host that the browser tests do not stub fails the request test, so the address is not `LINKS`.
    const siteBase = import.meta.env.VITE_SITE_BASE || '/';
</script>

<header>
    <a class="brand" href={ROUTES.home}><img src={`${siteBase}brand/app-icon.svg`} alt="" /><span>OpenBikeComputer</span></a>
    <nav aria-label="Pages">
        {#each links as link (link.href)}
            <a href={link.href} aria-current={link.current ? 'page' : undefined}>{link.label}</a>
        {/each}
    </nav>
    {#if device}
        <a class="chip" href={ROUTES.device} title={device.connected ? 'Connected' : 'Not connected'}>
            <span class="dot" class:connected={device.connected}></span>{device.name}
        </a>
        {#if device.sync === 'running'}
            <span class="sync turning" role="img" aria-label="Syncing"><Icon name="sync" /></span>
        {:else if device.sync === 'problem'}
            <button type="button" class="sync problem" aria-label="Sync again" title="The last sync had a problem. Sync again." onclick={onsync}><Icon name="sync" /></button>
        {/if}
    {/if}
    <a class="settings" href={ROUTES.settings} aria-label="Settings" aria-current={settings ? 'page' : undefined}><Icon name="gear" /></a>
</header>

<style>
    header {
        display: flex;
        align-items: center;
        gap: 28px;
        height: 48px;
        flex: none;
        padding: 0 20px;
        background: var(--rust);
        color: var(--cream);
    }
    .brand {
        display: flex;
        flex: none;
        align-items: center;
        gap: 10px;
        color: inherit;
        text-decoration: none;
    }
    .brand img {
        width: 28px;
        height: 28px;
        border-radius: 6px;
    }
    .brand span {
        font: 700 17px var(--mono);
    }
    nav {
        display: flex;
        align-items: center;
        gap: 24px;
        height: 100%;
        min-width: 0;
        overflow-x: auto;
        font-size: 13px;
        white-space: nowrap;
    }
    nav a {
        display: flex;
        align-items: center;
        height: 100%;
        color: inherit;
        text-decoration: none;
    }
    nav a:hover {
        text-decoration: underline;
        text-underline-offset: 4px;
    }
    nav a[aria-current="page"] {
        font-weight: 700;
        border-bottom: 3px solid var(--amber);
    }
    nav a[aria-current="page"]:hover { text-decoration: none; }
    .chip {
        display: flex;
        flex: none;
        align-items: center;
        gap: 7px;
        margin-left: auto;
        padding: 4px 10px;
        border: 1px solid color-mix(in srgb, var(--cream) 70%, transparent);
        border-radius: 6px;
        color: inherit;
        font-size: 13px;
        text-decoration: none;
    }
    .chip:hover {
        background: color-mix(in srgb, var(--rust) 80%, var(--on-amber));
    }
    .dot {
        width: 8px;
        height: 8px;
        border: 1.5px solid var(--cream);
        border-radius: 50%;
    }
    .dot.connected {
        border-color: var(--connected-on-rust);
        background: var(--connected-on-rust);
    }
    .sync {
        display: grid;
        flex: none;
        place-items: center;
        width: 32px;
        height: 32px;
        margin: 0 -16px 0 -8px;
        padding: 0;
        border: 0;
        border-radius: 6px;
        background: none;
        color: inherit;
    }
    .turning :global(svg) {
        animation: turn 1.2s linear infinite;
    }
    @keyframes turn {
        to { transform: rotate(360deg); }
    }
    @media (prefers-reduced-motion: reduce) {
        .turning :global(svg) { animation: none; }
    }
    .problem {
        position: relative;
        background: var(--cream);
        color: var(--rust);
    }
    /* The mark of a sync that needs the rider. */
    .problem::after {
        position: absolute;
        top: -3px;
        right: -3px;
        width: 8px;
        height: 8px;
        border: 2px solid var(--rust);
        border-radius: 50%;
        background: var(--cream);
        content: '';
    }
    .chip + .settings, .sync + .settings {
        margin-left: 0;
    }
    .settings {
        display: grid;
        flex: none;
        place-items: center;
        width: 32px;
        height: 32px;
        margin-left: auto;
        border-radius: 6px;
        color: inherit;
    }
    .settings:hover, .settings[aria-current="page"] {
        background: color-mix(in srgb, var(--rust) 80%, var(--on-amber));
    }
    @media (max-width: 1150px) {
        .brand span { font-size: 14px; }
    }
    @media (max-width: 760px) {
        header { gap: 12px; padding-inline: 12px; }
        .brand span { display: none; }
        nav { gap: 16px; }
    }
</style>
