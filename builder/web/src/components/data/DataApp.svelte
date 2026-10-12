<script lang="ts">
    import { onMount } from "svelte";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { SCREENS, screenAt } from "../../lib/data/screens";
    import { hardware, size, type Setup } from "../../lib/data/setup";
    import Failure from "./Failure.svelte";
    import Later from "./Later.svelte";
    import Machine from "./Machine.svelte";

    const setup = new CliQuery<Setup>(["setup"]);
    let hash = $state(location.hash);
    const screen = $derived(screenAt(hash));

    onMount(() => {
        const follow = () => (hash = location.hash);
        window.addEventListener("hashchange", follow);
        // A run fills the store, so the free space in the header is read again each minute.
        const stop = setup.poll(60_000);
        return () => {
            stop();
            window.removeEventListener("hashchange", follow);
        };
    });
</script>

<header style:background={setup.output?.machine.colour}>
    <b>OBC data</b>
    {#if setup.output}
        <span class="pill">This page runs on <b>{setup.output.machine.name}</b></span>
        <span class="facts">{hardware(setup.output.machine)} · store {size(setup.output.store.free_bytes)} free</span>
    {/if}
</header>

<nav aria-label="Main">
    {#each SCREENS as item (item.id)}
        <a href={item.hash} aria-current={item.id === screen.id ? "page" : undefined}>{item.label}</a>
    {/each}
</nav>

<main>
    {#if setup.result && !setup.result.ok}<Failure error={setup.result.error} />{/if}
    {#if screen.id === "machine"}
        <Machine {setup} />
    {:else}
        <Later {screen} />
    {/if}
</main>

<style>
    :global(:root) {
        --ready: #2e6b4a;
        --missing: #8a1c12;
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
    :global(h1),
    :global(h2) {
        margin: 0;
        font-weight: 600;
    }
    header {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 12px 20px;
        padding: 12px 24px;
        background: #262624;
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
