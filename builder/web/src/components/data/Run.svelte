<script lang="ts">
    import { onMount } from "svelte";
    import { command } from "../../lib/data/cli";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { changes, clock, span, title, type RunView, type Status } from "../../lib/data/shapes";
    import Action from "./Action.svelte";
    import Failure from "./Failure.svelte";
    import Phases from "./Phases.svelte";

    let { id, status }: { id: string; status: Status | null } = $props();

    // The parent mounts one Run for each id.
    // svelte-ignore state_referenced_locally
    const run = new CliQuery<RunView>(["runs", id]);
    let stop = () => {};
    let now = $state(Date.now());
    onMount(() => {
        stop = run.poll(2_000);
        const tick = setInterval(() => (now = Date.now()), 10_000);
        return () => {
            stop();
            clearInterval(tick);
        };
    });
    $effect(() => {
        if (run.output && run.output.state !== "running" && run.output.state !== "starting") stop();
    });

    let stopping = $state(false);
    const view = $derived(run.output);
    const target = $derived(view && changes(view.args));
    const where = $derived(view?.phases.find((phase) => phase.state === view.state && phase.state !== "ok")?.phase);
    const facts = $derived.by(() => {
        if (!view) return "";
        const facts = [`started ${clock(view.started)}`];
        if (view.state === "starting") facts.push("starting");
        else if (view.state === "running") facts.push(`${span(now - Date.parse(view.started))} so far`);
        else if (view.wall_ms !== null) facts.push(`${view.state === "ok" ? "took" : view.state} ${where ? `in ${where} ` : ""}after ${span(view.wall_ms)}`);
        if (target && (view.state === "failed" || view.state === "stopped") && !view.switched.length) facts.push(`${target} did not change`);
        const lock = status?.targets.find((at) => at.target === target);
        if (lock && !lock.lock && (view.state === "failed" || view.state === "stopped")) facts.push(`the ${target} lock is free`);
        return facts.join(" · ");
    });
    /** The review of the same update again: its args without the review that it did. */
    const resume = $derived.by(() => {
        if (!view || !target) return null;
        const args = view.args.filter((arg, i, all) => arg !== "--accept-format-change" && arg !== "--expect" && all[i - 1] !== "--expect");
        return [...args, "--dry-run"];
    });

    function progress(layer: RunView["layers"][number]): string {
        if (layer.done === layer.total) return "done";
        if (view?.state === "running") return layer.done || layer.leaves_baking.length ? `${layer.done} of ${layer.total}` : "waits";
        return layer.done ? `stopped at ${layer.done} of ${layer.total}` : "not started";
    }
</script>

{#if run.error}<Failure error={run.error} />{/if}

{#if view}
    <div class="title">
        <h1>{title(view.args)}</h1>
        <span class="soft">{facts}</span>
        <span class="grow"></span>
        {#if view.state === "running" && !stopping}
            <button type="button" class="stop" onclick={() => (stopping = true)}>Stop…</button>
        {/if}
    </div>

    {#if view.state === "running" && stopping}
        <section class="card confirm" aria-label="Stop the run">
            <p>The run stops before its next fetch, step or phase. What it finished stays in the store{target ? `, and ${target} does not change` : ""}.</p>
            <div class="row">
                <Action label="Stop the run" args={["runs", id, "--stop"]} ondone={(output: RunView) => run.ask()({ ok: true, output })} />
                <button type="button" onclick={() => (stopping = false)}>Keep it running</button>
            </div>
        </section>
    {/if}

    {#if view.error}
        <Failure error={view.error}>
            {#if view.error.kept.length}
                <p><b>Kept</b> · {new Intl.ListFormat("en").format(view.error.kept)}. The next update reuses them.</p>
            {/if}
            {#if resume}
                <p class="row">
                    <a class="button" href="#/{target}/resume/{id}">Resume…</a>
                    <span>A new review of the same update: <code class="resume">{command(resume)}</code></span>
                </p>
            {/if}
        </Failure>
    {/if}

    {#if view.phases.length}<Phases entered={view.phases} />{/if}

    {#if view.layers.length}
        <section class="card" aria-labelledby="layers">
            <h2 id="layers">Layers in this run</h2>
            {#each view.layers as layer (layer.layer)}
                <div class="layer">
                    <b>{layer.layer}</b>
                    <span class={layer.done === layer.total ? "ready" : "soft"}>{progress(layer)}</span>
                    <progress class:done={layer.done === layer.total} max={layer.total} value={layer.done} aria-label={layer.layer}></progress>
                    {#if layer.leaves_baking.length}
                        <span class="leaves">baking {layer.leaves_baking.slice(0, 12).join(" · ")}{layer.leaves_baking.length > 12 ? ` and ${layer.leaves_baking.length - 12} more` : ""}</span>
                    {/if}
                </div>
            {/each}
        </section>
    {/if}

    <section class="card" aria-labelledby="log">
        <h2 id="log">Log</h2>
        <p class="soft">The last {view.log.length} lines. In a terminal: <code>{command(["runs", id, "--follow"])}</code></p>
        {#if view.log.length}<pre>{view.log.join("\n")}</pre>{/if}
        <p class="soft">The run: <code>{command(view.args)}</code></p>
        {#if view.state === "running"}<p class="soft">Closing this page, the tunnel or the laptop does not stop the run.</p>{/if}
    </section>
{:else if !run.error}
    <p class="soft">Reading run {id}…</p>
{/if}

<style>
    .title,
    .row {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 8px 16px;
    }
    h1 {
        font-size: 28px;
        line-height: 36px;
    }
    h2 {
        font-size: 18px;
        line-height: 26px;
    }
    .grow {
        flex-grow: 1;
    }
    p {
        margin: 0;
    }
    .card {
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
    .confirm {
        border: 2px solid var(--missing);
    }
    button,
    .button {
        min-height: 44px;
        padding: 10px 20px;
        background: var(--panel);
        color: var(--ink);
        border: 1px solid var(--ink);
        border-radius: 8px;
        font: 600 15px var(--sans);
        text-decoration: none;
    }
    .stop {
        color: var(--missing);
        border-color: var(--missing);
    }
    .button {
        background: var(--ink);
        color: var(--panel);
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
    .leaves {
        flex-basis: 100%;
        font-family: var(--mono);
        font-size: 13px;
        color: var(--running);
    }
    pre {
        margin: 0;
        max-height: 320px;
        overflow: auto;
        padding: 12px;
        background: #1f1f1d;
        color: #f2f1ec;
        border-radius: 8px;
        font: 13px/20px var(--mono);
    }
    code {
        font-family: var(--mono);
        font-size: 13px;
    }
    .soft {
        color: var(--ink-soft);
    }
    .ready {
        color: var(--ready);
    }
</style>
