<script lang="ts">
    import { onMount } from "svelte";
    import { command } from "../../lib/data/cli";
    import { PHASES, span, type Holder } from "../../lib/data/shapes";
    import Phases from "./Phases.svelte";

    // A run on another machine, from the lock of the target that it changes. This page can only
    // watch it: its log and its Stop are on the page of its machine.
    let { target, holder }: { target: string; holder: Holder } = $props();

    /** How old a heartbeat is before `obc data unlock --from` may free the lock. */
    const STALE_MS = 600_000;

    let now = $state(Date.now());
    onMount(() => {
        const tick = setInterval(() => (now = Date.now()), 10_000);
        return () => clearInterval(tick);
    });
    const age = $derived(now - Date.parse(holder.heartbeat));
    const at = $derived(PHASES.indexOf(holder.phase));
    const entered = $derived(PHASES.slice(0, at + 1).map((phase, i) => ({ phase, state: i < at ? ("ok" as const) : ("running" as const) })));
</script>

<div class="title">
    <h1>Run {holder.run}</h1>
    <span class="pill">on {holder.machine}</span>
    <span class="soft">{target} · phase {holder.phase} · last heartbeat {span(age)} ago</span>
</div>

<section class="card watch">
    <p>You can watch this run here. To stop it or read its log, open the page of <b>{holder.machine}</b>:</p>
    <code>{command(["open", holder.machine])}</code>
    {#if age > STALE_MS}
        <p class="missing">No heartbeat for {span(age)}: the run may have stopped. Open its machine first.</p>
    {/if}
</section>

<Phases {entered} />

{#if holder.progress.length}
    <section class="card" aria-labelledby="layers">
        <h2 id="layers">Layers in this run</h2>
        {#each holder.progress as layer (layer.layer)}
            <div class="layer">
                <b>{layer.layer}</b>
                <span class={layer.done === layer.total ? "ready" : "soft"}>
                    {layer.done === layer.total ? "done" : layer.done ? `${layer.done} of ${layer.total}` : "waits"}
                </span>
                <progress class:done={layer.done === layer.total} max={layer.total} value={layer.done} aria-label={layer.layer}></progress>
            </div>
        {/each}
    </section>
{/if}

<p class="soft">From {target}/lock.json, which the run writes each minute. {target} does not change before the switch.</p>

<style>
    .title {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 8px 16px;
    }
    h1 {
        font-size: 28px;
        line-height: 36px;
        overflow-wrap: anywhere;
    }
    .pill {
        padding: 2px 12px;
        background: var(--ink);
        color: var(--panel);
        border-radius: 999px;
    }
    h2 {
        font-size: 18px;
        line-height: 26px;
    }
    .card {
        display: flex;
        flex-direction: column;
        gap: 8px;
        padding: 20px;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
        overflow-wrap: anywhere;
    }
    .watch {
        border: 2px solid var(--ink);
    }
    .layer {
        display: flex;
        flex-wrap: wrap;
        justify-content: space-between;
        gap: 2px 12px;
        padding: 4px 0;
    }
    progress {
        flex-basis: 100%;
        height: 8px;
        accent-color: var(--running);
    }
    progress.done {
        accent-color: var(--ready);
    }
    .ready {
        color: var(--ready);
    }
    p {
        margin: 0;
    }
    code {
        font-family: var(--mono);
        font-size: 13px;
    }
    .soft {
        color: var(--ink-soft);
    }
    .missing {
        color: var(--missing);
    }
</style>
