<script lang="ts">
    import { command } from "../../lib/data/cli";
    import { clock, PHASES, type Holder } from "../../lib/data/shapes";
    import Layers from "./Layers.svelte";
    import Phases from "./Phases.svelte";

    // A run on another machine, from the lock of the target that it changes. This page can only
    // watch it: its log and its Stop are on the page of its machine. `ended` is a run whose lock is
    // free again: `holder` is its last lock.
    let { target, holder, ended }: { target: string; holder: Holder; ended: boolean } = $props();

    const at = $derived(PHASES.indexOf(holder.phase));
    const entered = $derived(
        PHASES.slice(0, at + 1).map((phase, i) => (i < at ? { phase, state: "ok" as const } : { phase, state: "running" as const, waiting: holder.waiting })),
    );
</script>

<div class="title">
    <h1>Run {holder.run}</h1>
    <span class="pill">on {holder.machine}</span>
    <span class="soft">{target} · phase {holder.phase} · last heartbeat {clock(holder.heartbeat)}</span>
</div>

<section class="card watch">
    {#if ended}
        <p>The run on <b>{holder.machine}</b> ended. To see how, open {holder.machine}:</p>
    {:else}
        <p>You can watch this run here. To stop it or read its log, open the page of <b>{holder.machine}</b>:</p>
    {/if}
    <code>{command(["open", holder.machine])}</code>
    {#if holder.stale && !ended}
        <p class="missing">No heartbeat since {clock(holder.heartbeat)}: the run may have stopped. Open its machine first.</p>
    {/if}
</section>

{#if !ended}
    <Phases {entered} />
    {#if holder.progress.length}<Layers layers={holder.progress} running />{/if}
    <p class="soft">From {target}/lock.json, which the run writes each minute. {target} does not change before the switch.</p>
{/if}

<style>
    .pill {
        padding: 2px 12px;
        background: var(--ink);
        color: var(--panel);
        border-radius: 999px;
    }
    .watch {
        border: 2px solid var(--ink);
    }
</style>
