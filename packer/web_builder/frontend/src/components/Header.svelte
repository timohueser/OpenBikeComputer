<script lang="ts">
    import { LINKS } from "../lib/constants";
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
            <span class="crumb mono">map builder</span>
        </div>
        <nav>
            <a href="#/advanced">Advanced editor</a>
            <a href={LINKS.docs}>Docs</a>
            <a href={LINKS.simulator}>Simulator</a>
            <a href={LINKS.github}>GitHub</a>
        </nav>
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
        width: min(1180px, 100% - 32px);
        margin: 0 auto;
        height: 100%;
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 16px;
    }

    .brand {
        display: flex;
        align-items: baseline;
        gap: 9px;
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

    nav {
        display: flex;
        gap: 18px;
        font-size: 13.5px;
    }

    nav a:first-child {
        color: var(--ink);
        font-weight: 600;
    }

    @media (max-width: 700px) {
        .crumb {
            display: none;
        }

        nav {
            gap: 12px;
        }
    }
</style>
