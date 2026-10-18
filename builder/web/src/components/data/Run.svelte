<script lang="ts">
    import { onMount } from "svelte";
    import { command } from "../../lib/data/cli";
    import { flagsOf, reviewArgs } from "../../lib/data/flags";
    import { leafSquare } from "../../lib/data/leaf";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { changes, clock, free, list, size, span, title, type RunView, type Status } from "../../lib/data/shapes";
    import Action from "./Action.svelte";
    import DataMap from "./DataMap.svelte";
    import Failure from "./Failure.svelte";
    import Layers from "./Layers.svelte";
    import Phases from "./Phases.svelte";
    import Timeline from "./Timeline.svelte";

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

    const view = $derived(run.output);
    const live = $derived(view?.state === "running" || view?.state === "starting");
    $effect(() => {
        if (view && !live) stop();
    });

    /** `ask` while the confirm shows; `asked` after `--stop` succeeded, until the run ends. */
    let stopping = $state<"no" | "ask" | "asked">("no");
    const target = $derived(view && changes(view.args));
    const where = $derived(view?.phases.find((phase) => phase.state === view.state && phase.state !== "ok")?.phase);
    const facts = $derived.by(() => {
        if (!view) return "";
        const facts = [`started ${clock(view.started)}`];
        if (view.state === "starting") facts.push("starting");
        else if (view.state === "running") facts.push(`${span(now - Date.parse(view.started))} so far`);
        else if (view.wall_ms !== null) facts.push(`${view.state === "ok" ? "took" : view.state} ${where ? `in ${where} ` : ""}after ${span(view.wall_ms)}`);
        if (target && (view.state === "failed" || view.state === "stopped") && !view.switched.length) facts.push(`${target} did not change`);
        const changed = status?.targets.find((at) => at.target === target);
        if (changed && !changed.lock && (view.state === "failed" || view.state === "stopped")) facts.push(`the ${target} lock is free`);
        if (changed?.lock?.died && changed.lock.run === id) facts.push(`it kept the ${target} lock, which the next update takes over`);
        return facts.join(" · ");
    });
    // The cell map: the leaves of one tiled layer that baked and that bake now.
    const tiled = $derived(view?.layers.filter((layer) => layer.leaves_done.length || layer.leaves_baking.length) ?? []);
    let chosen = $state<string | null>(null);
    const mapped = $derived(tiled.find((layer) => layer.layer === chosen) ?? tiled[0]);
    const cells = $derived(
        [...(mapped?.leaves_done ?? []).map((leaf) => [leaf, "done"]), ...(mapped?.leaves_baking ?? []).map((leaf) => [leaf, "baking"])].flatMap(
            ([leaf, state]) => {
                const bounds = leafSquare(leaf);
                return bounds ? [{ bounds, color: state === "done" ? "var(--ready)" : "var(--running)", label: `${leaf} · ${state}` }] : [];
            },
        ),
    );
    /** The review of the same update again: the settings flags of the run. */
    const resume = $derived(view?.args[0] === "update" && target ? reviewArgs(target, flagsOf(view.args)) : null);
</script>

{#if run.error}<Failure error={run.error} />{/if}

{#if view}
    <div class="title">
        <h1>{title(view.args)}</h1>
        <span class="soft">{facts}</span>
        <span class="grow"></span>
        {#if target && view.args.includes("--plan")}<a class="primary" href="#/{target}/review?plan={encodeURIComponent(id)}">Open the plan</a>{/if}
        {#if live && stopping === "no"}
            <button type="button" class="stop" onclick={() => (stopping = "ask")}>Stop…</button>
        {/if}
    </div>

    {#if live && stopping === "ask"}
        <section class="card confirm" aria-label="Stop the run">
            <p>
                The run stops before its next fetch, step or phase. What it finished stays in the store{target && !view.switched.length
                    ? `, and ${target} does not change`
                    : ""}.
            </p>
            <div class="title">
                <Action
                    label="Stop the run"
                    args={["runs", id, "--stop"]}
                    ondone={(output: RunView) => {
                        stopping = "asked";
                        run.ask()({ ok: true, output });
                    }}
                />
                <button type="button" onclick={() => (stopping = "no")}>Keep it running</button>
            </div>
        </section>
    {:else if live && stopping === "asked"}
        <p class="running">Stopping: the run stops before its next fetch, step or phase.</p>
    {/if}

    {#if view.error}
        <Failure error={view.error}>
            {#if view.error.kept.length}
                <p><b>Kept</b> · {list(view.error.kept)}. The next update reuses them.</p>
            {/if}
            {#if resume}
                <p class="title">
                    <a class="primary" href="#/{target}/resume/{id}">Resume…</a>
                    <a href="#/machine">Clean the store…</a>
                    <span>A new review of the same update: <code class="resume">{command(resume)}</code></span>
                </p>
            {/if}
        </Failure>
    {/if}

    {#if view.phases.length}<Phases entered={view.phases} />{/if}

    <Timeline {view} {now} />

    {#if live && status}
        {@const machine = status.machine}
        <p class="soft">
            {machine.name} · load {machine.load?.toFixed(1) ?? "unknown"} on {machine.cores} cores · RAM {machine.memory_available_bytes === null
                ? "use unknown"
                : `${size(machine.memory_available_bytes)} free`} · store {free(machine.store_free_bytes)}
        </p>
    {/if}

    {#if view.layers.length}<Layers layers={view.layers} running={live} {now} />{/if}

    {#if tiled.length}
        <section class="card" aria-labelledby="cells">
            <div class="title">
                <h2 id="cells">Cells of {mapped?.layer}</h2>
                {#if tiled.length > 1}
                    <select value={mapped?.layer} aria-label="Layer" onchange={(event) => (chosen = event.currentTarget.value)}>{#each tiled as layer (layer.layer)}<option value={layer.layer}>{layer.layer}</option>{/each}</select>
                {/if}
            </div>
            <DataMap {cells} />
        </section>
    {/if}

    <section class="card" aria-labelledby="log">
        <h2 id="log">Log</h2>
        <p class="soft">The last {view.log.length} lines of <code>{view.log_path}</code>. In a terminal: <code>{command(["runs", id, "--follow"])}</code></p>
        {#if view.log.length}<pre>{view.log.join("\n")}</pre>{/if}
        <p class="soft">The run: <code>{command(view.args)}</code></p>
        {#if live}<p class="soft">Closing this page, the tunnel or the laptop does not stop the run.</p>{/if}
    </section>
{:else if !run.error}
    <p class="soft">Reading run {id}…</p>
{/if}

<style>
    .confirm {
        border: 2px solid var(--missing);
    }
    .stop {
        color: var(--missing);
        border-color: var(--missing);
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
</style>
