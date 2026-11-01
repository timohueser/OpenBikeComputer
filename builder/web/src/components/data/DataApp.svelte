<script lang="ts">
    import { onMount } from "svelte";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { runAt, SCREENS, screenAt } from "../../lib/data/screens";
    import { elsewhere, free, hardware, title, type RunList, type Status } from "../../lib/data/shapes";
    import { appearance, startTheme, toggleTheme } from "../../lib/theme.svelte";
    import Icon from "../planner/PlannerIcon.svelte";
    import Bake from "./Bake.svelte";
    import Failure from "./Failure.svelte";
    import Machine from "./Machine.svelte";
    import Overview from "./Overview.svelte";
    import Runs from "./Runs.svelte";
    import Sources from "./Sources.svelte";
    import TargetScreen from "./Target.svelte";

    const status = new CliQuery<Status>([]);
    const runs = new CliQuery<RunList>(["runs"]);
    const machine = $derived(status.output?.machine ?? null);
    const here = $derived(runs.output?.runs.filter((run) => run.state === "running" || run.state === "starting") ?? []);
    const others = $derived(elsewhere(status.output));
    let hash = $state(location.hash);
    const screen = $derived(screenAt(hash));
    // The Bake tab opens the last bake again, also after a prepare or a run that it linked to.
    let lastBake = $state("#/");
    $effect(() => {
        if (screen.id === "bake") lastBake = hash || "#/";
    });

    onMount(startTheme);
    onMount(() => {
        const follow = () => (hash = location.hash);
        window.addEventListener("hashchange", follow);
        // A run fills the store, so the free space in the header is read again each minute. A lock
        // of another machine has a heartbeat each minute.
        const stops = [status.poll(60_000), runs.poll(10_000)];
        return () => {
            stops.forEach((stop) => stop());
            window.removeEventListener("hashchange", follow);
        };
    });
</script>

<header class="site" style:background={machine?.colour}>
    <a class="brand" href="#/">
        <svg viewBox="0 0 128 128" aria-hidden="true">
            <rect width="128" height="128" rx="24" fill="#f4f2eb" />
            <g transform="translate(6.5 8.8) scale(1.15)">
                <path fill="#5c5a2e" d="M44 10H56V91H44Z" />
                <path fill="#f4a81d" d="M13.1 26.4 27.8 16.4l50.9 3.6-1.5 21.9-50.9-3.6Z" />
                <path fill="#f4a81d" d="M21.3 54 73.2 50.4l14.7 10-13.2 11.9-51.9 3.6Z" />
            </g>
        </svg>
        <span>OBC data</span>
    </a>
    <nav aria-label="Main">
        {#each SCREENS as item (item.id)}
            <a href={item.id === "bake" ? lastBake : item.hash} aria-current={item.id === screen.id ? "page" : undefined}>{item.label}</a>
        {/each}
    </nav>
    <button type="button" class="theme" aria-label={appearance.dark ? "Use light theme" : "Use dark theme"} onclick={toggleTheme}>
        <Icon name={appearance.dark ? "sun" : "moon"} />
    </button>
</header>

<div class="machine" aria-label="This machine">
    {#if machine}
        <span class="here"><span class="dot" style:background={machine.colour}></span>This page runs on <b>{machine.name}</b></span>
        <span class="soft">{hardware(machine)} · store {free(machine.store_free_bytes)}</span>
        <span class="grow"></span>
        <span class="runs">
            <span class="soft">Runs here</span>
            {#each here as run (run.id)}<a class="run" href="#/runs/{run.id}"><span class="pulse"></span>{title(run.args)}</a>{:else}<span>none</span>{/each}
        </span>
        <span class="runs">
            <span class="soft">Elsewhere</span>
            {#each others as { target, holder } (holder.run)}<a class="run" href="#/runs/{holder.run}"><span class="pulse"></span>{holder.machine} · {target} · {holder.phase}</a>{:else}<span>none</span>{/each}
        </span>
    {:else}
        <span class="soft">Reading this machine…</span>
    {/if}
</div>

<main>
    {#if status.error}<Failure error={status.error} />{/if}
    {#if screen.id === "bake"}
        <Bake {hash} status={status.output} />
    {:else if screen.id === "machine"}
        <Machine {machine} />
    {:else if screen.id === "runs"}
        <Runs id={runAt(hash)} {runs} {status} />
    {:else if screen.id === "overview"}
        <Overview {status} />
    {:else if screen.id === "prod" || screen.id === "local"}
        {#key screen.id}<TargetScreen name={screen.id} {status} {runs} />{/key}
    {:else}
        <Sources {hash} {status} />
    {/if}
</main>

<style>
    /* The header of the route planner, in the colour of the machine: a glance tells which machine
       the page acts on. */
    .site {
        position: sticky;
        top: 0;
        z-index: 1000;
        display: flex;
        align-items: center;
        gap: 28px;
        height: 48px;
        padding: 0 20px;
        /* No machine has this grey: the header has it only until the first answer. */
        background: #6e6c66;
        color: #fff;
    }
    .brand {
        display: flex;
        flex: none;
        align-items: center;
        gap: 10px;
        color: inherit;
        font: 700 16px var(--mono);
        text-decoration: none;
    }
    .brand svg {
        width: 28px;
        height: 28px;
        border-radius: 6px;
    }
    nav {
        display: flex;
        align-self: stretch;
        gap: 22px;
        min-width: 0;
        overflow-x: auto;
        scrollbar-width: none;
        font-size: 13px;
        white-space: nowrap;
    }
    nav a {
        display: flex;
        align-items: center;
        color: inherit;
        text-decoration: none;
        border-block: 3px solid transparent;
    }
    nav a:hover {
        border-bottom-color: rgb(255 255 255 / 0.35);
    }
    nav a[aria-current="page"] {
        font-weight: 700;
        border-bottom-color: var(--amber);
    }
    .site :focus-visible {
        outline-color: #fff;
    }
    .theme {
        display: grid;
        flex: none;
        place-items: center;
        width: 32px;
        height: 32px;
        margin-left: auto;
        padding: 0;
        background: none;
        border: 0;
        border-radius: 6px;
        color: inherit;
    }
    .theme:hover {
        background: rgb(255 255 255 / 0.12);
    }
    /* The trip bar of the planner: what this machine is and runs. */
    .machine {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 6px 20px;
        min-height: 44px;
        padding: 8px 20px;
        background: var(--panel);
        border-bottom: 1px solid var(--line);
        font-size: 13px;
    }
    .here {
        display: inline-flex;
        align-items: center;
        gap: 8px;
    }
    .dot {
        width: 10px;
        height: 10px;
        border-radius: 999px;
        background: #6e6c66;
        box-shadow: 0 0 0 2px color-mix(in srgb, var(--ink) 12%, transparent);
    }
    .grow {
        flex-grow: 1;
    }
    .runs {
        display: inline-flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 4px 10px;
    }
    .run {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        color: var(--running);
        font-weight: 600;
        text-decoration: none;
    }
    .run:hover {
        text-decoration: underline;
        text-underline-offset: 3px;
    }
    .pulse {
        width: 8px;
        height: 8px;
        border-radius: 999px;
        background: currentColor;
        animation: pulse 1.6s ease-out infinite;
    }
    @keyframes pulse {
        0% {
            box-shadow: 0 0 0 0 color-mix(in srgb, currentColor 45%, transparent);
        }
        100% {
            box-shadow: 0 0 0 7px transparent;
        }
    }
    @media (prefers-reduced-motion: reduce) {
        .pulse {
            animation: none;
        }
    }
    main {
        max-width: 1320px;
        margin: 0 auto;
        padding: 24px 20px 48px;
        display: flex;
        flex-direction: column;
        gap: 16px;
    }
    @media (max-width: 760px) {
        .site {
            flex-wrap: wrap;
            gap: 0 16px;
            height: auto;
            padding: 0 16px;
        }
        .brand {
            min-height: 44px;
        }
        nav {
            order: 3;
            width: 100%;
            min-height: 40px;
            gap: 18px;
        }
        .machine,
        main {
            padding-inline: 16px;
        }
    }
</style>
