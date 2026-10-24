<script lang="ts">
    import { onMount } from "svelte";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { list, PHASES, size, span, type RunStep, type RunView, type Status, type Target } from "../../lib/data/shapes";
    import Action from "./Action.svelte";
    import Failure from "./Failure.svelte";
    import Timeline from "./Timeline.svelte";

    // A bake as it runs: where it is, how long is left against the estimate of its plan, and each
    // step with its time, CPU, peak memory and output, slowest first.
    let {
        run,
        plan,
        kind,
        status,
        again,
        args,
    }: {
        run: string;
        plan: string | null;
        kind: Target["kind"];
        status: Status | null;
        again: () => void;
        /** Gets the arguments of the run once it is read. */
        args: (args: string[]) => void;
    } = $props();

    // svelte-ignore state_referenced_locally
    const bake = new CliQuery<RunView>(["runs", run]);
    // svelte-ignore state_referenced_locally
    const planned = plan ? new CliQuery<RunView>(["runs", plan]) : null;
    let stop = () => {};
    let now = $state(Date.now());
    onMount(() => {
        stop = bake.poll(2_000);
        void planned?.refresh();
        const tick = setInterval(() => (now = Date.now()), 1_000);
        return () => {
            stop();
            clearInterval(tick);
        };
    });
    const view = $derived(bake.output);
    const live = $derived(view?.state === "running" || view?.state === "starting");
    $effect(() => {
        if (view && !live) stop();
    });
    let told = false;
    $effect(() => {
        if (view && !told) {
            told = true;
            args(view.args);
        }
    });

    // A run whose process died has no wall time: it ran until the last step or fetch that ended.
    const ended = $derived([...(view?.steps ?? []), ...(view?.fetches ?? [])].reduce((last, item) => Math.max(last, item.end_ms ?? 0), 0));
    const elapsed = $derived(view ? (live ? now - Date.parse(view.started) : (view.wall_ms ?? ended)) : 0);
    const estimate = $derived(planned?.output?.review?.cost.wall_ms ?? null);
    const phases = $derived(PHASES.filter((phase) => kind === "r2" || phase !== "router-ready"));
    const entered = (phase: string) => view?.phases.find((at) => at.phase === phase);
    const worked = $derived(view?.steps.filter((step) => !step.reused && !step.unchanged) ?? []);
    const running = $derived(worked.filter((step) => step.start_ms != null && step.end_ms == null && !step.error));
    const finished = $derived(worked.filter((step) => step.wall_ms !== null).sort((a, b) => (b.wall_ms ?? 0) - (a.wall_ms ?? 0)));
    const kept = $derived((view?.steps.length ?? 0) - worked.length);
    let all = $state(false);
    const shown = $derived(all ? finished : finished.slice(0, 20));
    let stopping = $state(false);

    const cores = (step: RunStep) => (step.cpu_ms != null && step.wall_ms ? `${(step.cpu_ms / step.wall_ms).toFixed(1)}×` : "");
    const headline = $derived.by(() => {
        if (!view) return "";
        if (view.state === "starting") return "Starting";
        if (live) {
            const left = estimate === null ? "" : estimate > elapsed ? ` · about ${span(estimate - elapsed)} left` : ` · past the plan's ${span(estimate)}`;
            return `Baking · ${span(elapsed)} so far${left}`;
        }
        if (view.state === "ok") return `Done in ${span(elapsed)}${estimate === null ? "" : ` · the plan said ${span(estimate)}`}`;
        return `${view.state === "failed" ? "Failed" : "Stopped"}${elapsed ? ` after ${span(elapsed)}` : ""}`;
    });
</script>

{#if bake.error}<Failure error={bake.error} />{/if}

{#if view}
    <div class="title">
        <b class={live ? "running" : view.state === "ok" ? "ready" : "missing"}>{headline}</b>
        <span class="grow"></span>
        <a href="#/runs/{run}">Log and details</a>
        {#if live && !stopping}<button type="button" class="stop" onclick={() => (stopping = true)}>Stop…</button>{/if}
    </div>

    {#if live && stopping}
        <div class="confirm">
            <p>The bake stops before its next step. What it finished stays, and the next bake reuses it.</p>
            <div class="title">
                <Action label="Stop the bake" args={["runs", run, "--stop"]} folded ondone={() => void bake.refresh()} />
                <button type="button" onclick={() => (stopping = false)}>Keep baking</button>
            </div>
        </div>
    {/if}

    <ol class="phases" aria-label="Phases">
        {#each phases as phase (phase)}
            {@const at = entered(phase)}
            <li data-state={at ? (at.state === "running" || at.state === "starting" ? "now" : at.state) : "next"}>{phase.replace("-", " ")}</li>
        {/each}
    </ol>

    {#if view.error}
        <Failure error={view.error}>
            {#if view.error.kept.length}<p><b>Kept</b> · {list(view.error.kept)}. The next bake reuses them.</p>{/if}
            <p><button type="button" class="primary" onclick={again}>Plan it again</button></p>
        </Failure>
    {/if}

    {#if live && status}
        {@const machine = status.machine}
        <p class="soft">
            {machine.name} now · {machine.memory_available_bytes === null ? "free memory unknown" : `${size(machine.memory_available_bytes)} memory free`} ·
            load {machine.load?.toFixed(1) ?? "unknown"} on {machine.cores} cores · {machine.store_free_bytes === null ? "" : `${size(machine.store_free_bytes)} disk free`}
        </p>
    {/if}

    <Timeline {view} {now} />

    {#if running.length}
        <section aria-labelledby="now">
            <h3 id="now">Running</h3>
            <ul class="running-steps">
                {#each running as step (step.step)}
                    <li><code>{step.step}</code><span class="soft">{span(now - Date.parse(view.started) - (step.start_ms ?? 0))}{step.last_wall_ms ? ` · took ${span(step.last_wall_ms)} last time` : ""}</span></li>
                {/each}
            </ul>
        </section>
    {/if}

    {#if finished.length}
        <section aria-labelledby="steps">
            <h3 id="steps">Steps, slowest first</h3>
            <p class="soft">{finished.length} done{running.length ? ` · ${running.length} running` : ""}{kept ? ` · ${kept} kept from earlier bakes` : ""}</p>
            <div class="table">
                <table>
                    <thead><tr><th>Step</th><th class="num">Time</th><th class="num">CPU</th><th class="num">Peak memory</th><th class="num">Output</th></tr></thead>
                    <tbody>
                        {#each shown as step (step.step)}
                            <tr>
                                <td><code>{step.step}</code></td>
                                <td class="num">{span(step.wall_ms ?? 0)}</td>
                                <td class="num">{step.cpu_ms == null ? "" : span(step.cpu_ms)} <span class="soft">{cores(step)}</span></td>
                                <td class="num">{step.peak_rss_bytes === null ? "" : size(step.peak_rss_bytes)}</td>
                                <td class="num">{step.bytes_out === null ? "" : size(step.bytes_out)}</td>
                            </tr>
                        {/each}
                    </tbody>
                </table>
            </div>
            {#if finished.length > shown.length}<button type="button" onclick={() => (all = true)}>Show all {finished.length}</button>{/if}
        </section>
    {/if}
{:else if !bake.error}
    <p class="soft">Reading the bake…</p>
{/if}

<style>
    .phases {
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .phases li {
        padding: 2px 10px;
        border: 1px solid var(--line-strong);
        border-radius: 999px;
        color: var(--ink-faint);
        font-size: 13px;
    }
    .phases li[data-state="ok"] {
        border-color: var(--ready);
        color: var(--ready);
    }
    .phases li[data-state="now"] {
        border-color: var(--running);
        background: var(--running);
        color: var(--panel);
    }
    .phases li:is([data-state="failed"], [data-state="stopped"]) {
        border-color: var(--missing);
        color: var(--missing);
    }
    h3 {
        margin: 0 0 6px;
        font-size: 15px;
    }
    .running-steps {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .running-steps li {
        display: flex;
        flex-wrap: wrap;
        justify-content: space-between;
        gap: 4px 12px;
        padding: 4px 0;
        border-bottom: 1px solid var(--line);
    }
    .num {
        text-align: right;
        white-space: nowrap;
        font-variant-numeric: tabular-nums;
    }
    .confirm {
        display: flex;
        flex-direction: column;
        gap: 8px;
        padding: 12px 16px;
        border: 2px solid var(--missing);
        border-radius: 8px;
    }
    .stop {
        color: var(--missing);
        border-color: var(--missing);
    }
</style>
