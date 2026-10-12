<script lang="ts">
    import { onMount } from "svelte";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { runAt, SCREENS, screenAt } from "../../lib/data/screens";
    import { elsewhere, free, hardware, title, type RunList, type Status } from "../../lib/data/shapes";
    import Failure from "./Failure.svelte";
    import Later from "./Later.svelte";
    import Machine from "./Machine.svelte";
    import Runs from "./Runs.svelte";

    const status = new CliQuery<Status>([]);
    const runs = new CliQuery<RunList>(["runs"]);
    const machine = $derived(status.output?.machine ?? null);
    const here = $derived(runs.output?.runs.filter((run) => run.state === "running" || run.state === "starting") ?? []);
    const others = $derived(elsewhere(status.output));
    let hash = $state(location.hash);
    const screen = $derived(screenAt(hash));

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

<header style:background={machine?.colour}>
    <b>OBC data</b>
    {#if machine}
        <span class="pill">This page runs on <b>{machine.name}</b></span>
        <span class="facts">{hardware(machine)} · store {free(machine.store_free_bytes)}</span>
        <span class="grow"></span>
        <span>
            Runs here:
            {#each here as run, i (run.id)}{i ? ", " : ""}<a href="#/runs/{run.id}">{title(run.args)}</a>{:else}none{/each}
        </span>
        <span>
            Elsewhere:
            {#each others as { target, holder }, i (holder.run)}{i ? ", " : ""}<a href="#/runs/{holder.run}"
                    >{holder.machine} · {target} · {holder.phase}</a
                >{:else}none{/each}
        </span>
    {/if}
</header>

<nav aria-label="Main">
    {#each SCREENS as item (item.id)}
        <a href={item.hash} aria-current={item.id === screen.id ? "page" : undefined}>{item.label}</a>
    {/each}
</nav>

<main>
    {#if status.error}<Failure error={status.error} />{/if}
    {#if screen.id === "machine"}
        <Machine {machine} />
    {:else if screen.id === "runs"}
        <Runs id={runAt(hash)} {runs} {status} />
    {:else}
        <Later {screen} />
    {/if}
</main>

<style>
    :global(:root) {
        --ready: #2e6b4a;
        --missing: #8a1c12;
        --running: #2c4fb8;
    }
    :global(body) {
        margin: 0;
        background: var(--parchment);
        color: var(--ink);
        font-family: var(--sans);
        font-size: 15px;
        line-height: 22px;
    }
    :global(*) {
        box-sizing: border-box;
    }
    /* What every screen shares. */
    main :global(:is(h1, h2)) {
        margin: 0;
        font-weight: 600;
        overflow-wrap: anywhere;
    }
    main :global(h1) {
        font-size: 28px;
        line-height: 36px;
    }
    main :global(h2) {
        font-size: 18px;
        line-height: 26px;
    }
    main :global(p) {
        margin: 0;
    }
    main :global(code) {
        font-family: var(--mono);
        font-size: 13px;
        overflow-wrap: anywhere;
    }
    main :global(.title) {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 8px 16px;
    }
    main :global(.grow) {
        flex-grow: 1;
    }
    main :global(.card) {
        display: flex;
        flex-direction: column;
        gap: 8px;
        min-width: 0;
        padding: 20px;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
        overflow-wrap: anywhere;
    }
    main :global(button) {
        min-height: 44px;
        padding: 0 20px;
        background: var(--panel);
        color: var(--ink);
        border: 1px solid var(--ink);
        border-radius: 8px;
        font: 600 15px var(--sans);
    }
    main :global(button:disabled) {
        opacity: 0.6;
        cursor: progress;
    }
    main :global(.soft) {
        color: var(--ink-soft);
    }
    main :global(:is(.ok, .ready)) {
        color: var(--ready);
    }
    main :global(:is(.starting, .running)) {
        color: var(--running);
    }
    main :global(:is(.failed, .stopped, .missing)) {
        color: var(--missing);
    }
    header {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 12px 20px;
        padding: 12px 24px;
        /* No machine has this grey: the header has it only until the first answer. */
        background: #6e6c66;
        color: #fff;
    }
    .pill {
        padding: 2px 12px;
        border: 1px solid rgb(255 255 255 / 0.5);
        border-radius: 999px;
    }
    .facts {
        color: rgb(255 255 255 / 0.85);
    }
    header .grow {
        flex-grow: 1;
    }
    header a {
        color: inherit;
        font-weight: 600;
    }
    nav {
        display: flex;
        flex-wrap: wrap;
        gap: 4px;
        padding: 0 16px;
        background: var(--panel);
        border-bottom: 1px solid var(--parchment-3);
    }
    nav a {
        padding: 12px;
        color: var(--ink);
        text-decoration: none;
        border-bottom: 3px solid transparent;
    }
    nav a[aria-current="page"] {
        font-weight: 600;
        border-bottom-color: var(--ink);
    }
    main {
        max-width: 1392px;
        margin: 0 auto;
        padding: 24px;
        display: flex;
        flex-direction: column;
        gap: 16px;
    }
    @media (max-width: 600px) {
        header,
        main {
            padding-inline: 16px;
        }
        nav {
            padding-inline: 4px;
        }
    }
</style>
