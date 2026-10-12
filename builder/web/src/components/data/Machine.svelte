<script lang="ts">
    import type { CliQuery } from "../../lib/data/query.svelte";
    import { hardware, size, type Readiness, type Setup } from "../../lib/data/setup";
    import Action from "./Action.svelte";

    let { setup }: { setup: CliQuery<Setup> } = $props();

    const MARK: Record<Readiness, string> = { ready: "✓", missing: "✗", optional: "○" };
    const report = $derived(setup.output);
    const missing = $derived(report?.tools.filter((tool) => tool.state === "missing").map((tool) => tool.name) ?? []);
    const checked = $derived(setup.at?.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hourCycle: "h23" }));
</script>

{#snippet mark(state: Readiness)}<span class="mark {state}" role="img" aria-label={state}>{MARK[state]}</span>{/snippet}

<div class="title">
    <h1>This machine{report ? `: ${report.machine.name}` : ""}</h1>
    {#if report}<span class="soft">{hardware(report.machine)} · checked at {checked}</span>{/if}
    <span class="grow"></span>
    <Action args={["setup"]} label="Check again" ondone={(output: Setup) => setup.accept({ ok: true, output })} />
</div>

{#if report}
    <section class="card can" aria-labelledby="can">
        <h2 id="can">What this machine can do</h2>
        {#each report.jobs as job (job.name)}
            <div class="row">{@render mark(job.state)}<b class="name">{job.name}</b><span class="wide {job.state}">{job.reason}</span></div>
        {/each}
    </section>

    <div class="pair">
        <section class="card" aria-labelledby="tools">
            <h2 id="tools">Tools</h2>
            {#each report.tools as tool (tool.name)}
                <div class="row line">
                    {@render mark(tool.state)}<b class="name">{tool.name}</b><span class="wide soft">{tool.needed_for}</span>
                    <code class={tool.state}>{tool.detail}</code>
                </div>
            {/each}
        </section>

        <section class="card" aria-labelledby="credentials">
            <h2 id="credentials">Credentials</h2>
            <p class="soft">Only whether each is set. Values stay in tools/obc.local or your home folder and never show here.</p>
            {#each report.credentials as credential (credential.name)}
                <div class="row line">{@render mark(credential.state)}<b class="name">{credential.name}</b><span class="wide soft">{credential.needed_for}</span></div>
            {/each}
        </section>
    </div>

    <div class="pair">
        <section class="card" aria-labelledby="store">
            <h2 id="store">Store</h2>
            <p><code>{report.store.path}</code> · {size(report.store.used_bytes)} used · {size(report.store.free_bytes)} free</p>
        </section>

        <section class="card" aria-labelledby="first">
            <h2 id="first">First steps</h2>
            <ol>
                <li>
                    {#if missing.length}
                        Install {new Intl.ListFormat("en").format(missing)} (the commands are under Tools), then press Check again.
                    {:else}
                        The tools are ready.
                    {/if}
                </li>
                <li>Open <a href="#/local">local</a>, choose a small area (the Freiburg preset is quick), and start <b>Update local</b>. It takes a few minutes.</li>
                <li>Start <b>Serve</b> and open the web planner or the map builder from there.</li>
                <li>Read the <a href="#/">Overview</a>: sources on the left, layers in the middle, targets on the right.</li>
            </ol>
        </section>
    </div>
{:else if !setup.result}
    <p class="soft">Checking this machine…</p>
{/if}

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
    }
    h2 {
        font-size: 18px;
        line-height: 26px;
        margin-bottom: 6px;
    }
    .grow {
        flex-grow: 1;
    }
    .card {
        display: flex;
        flex-direction: column;
        gap: 4px;
        padding: 20px;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
    }
    .can {
        border: 2px solid var(--ink);
    }
    .pair {
        display: flex;
        flex-wrap: wrap;
        gap: 16px;
    }
    .pair > .card {
        flex: 1 1 420px;
        min-width: 0;
    }
    .row {
        display: flex;
        flex-wrap: wrap;
        align-items: baseline;
        gap: 4px 12px;
        min-height: 32px;
        padding: 4px 0;
    }
    .line {
        border-bottom: 1px solid var(--line);
    }
    .mark {
        width: 18px;
        font-weight: 600;
    }
    .name {
        flex: 0 1 200px;
        min-width: 0;
        overflow-wrap: anywhere;
    }
    .can .name {
        flex-basis: 300px;
    }
    .wide {
        flex: 1 1 140px;
        min-width: 0;
    }
    p,
    ol {
        margin: 0;
    }
    ol {
        padding-left: 22px;
        display: flex;
        flex-direction: column;
        gap: 6px;
    }
    code {
        font-family: var(--mono);
        font-size: 13px;
        overflow-wrap: anywhere;
    }
    .soft {
        color: var(--ink-soft);
    }
    .ready {
        color: var(--ready);
    }
    .missing {
        color: var(--missing);
    }
    .optional {
        color: var(--ink-faint);
    }
</style>
