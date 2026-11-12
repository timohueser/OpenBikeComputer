<script lang="ts">
    import Icon from './planner/PlannerIcon.svelte';
    import { ROUTES } from '../lib/routes';
    import { appearance, toggleTheme } from '../lib/theme.svelte';

    let { links }: { links: { href: string; label: string; current: boolean }[] } = $props();
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
    <button type="button" class="theme" aria-label={appearance.dark ? 'Use light theme' : 'Use dark theme'} onclick={toggleTheme}>
        <Icon name={appearance.dark ? 'sun' : 'moon'} />
    </button>
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
    .theme {
        display: grid;
        flex: none;
        place-items: center;
        width: 32px;
        height: 32px;
        margin-left: auto;
        padding: 0;
        border: 0;
        border-radius: 6px;
        background: none;
        color: inherit;
    }
    .theme:hover {
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
