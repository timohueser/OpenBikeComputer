<script lang="ts">
    import { onMount } from "svelte";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { free, hardware, size, type Machine, type Readiness, type Setup } from "../../lib/data/shapes";
    import Action from "./Action.svelte";
    import Failure from "./Failure.svelte";

    let { machine }: { machine: Machine | null } = $props();

    const setup = new CliQuery<Setup>(["setup"]);
    onMount(() => void setup.refresh());

    const MARK: Record<Readiness, string> = { ready: "✓", missing: "✗", optional: "○" };
    const report = $derived(setup.output);
    const missing = $derived(report?.tools.filter((tool) => tool.state === "missing").map((tool) => tool.name) ?? []);
    const checked = $derived(setup.at?.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hourCycle: "h23" }));
    const facts = $derived([machine && hardware(machine), checked && `checked at ${checked}`].filter(Boolean).join(" · "));
</script>

<!-- A name breaks only at a space or after `_` or `/`, never inside a word. -->
{#snippet head(state: Readiness, name: string)}
    <span class="head">
        <span class="mark {state}" role="img" aria-label={state}>{MARK[state]}</span>
        <b class="name">{#each name.split(/(?<=[_/])/) as part}{part}<wbr />{/each}</b>
    </span>
{/snippet}

<div class="title">
    <h1>This machine{machine ? `: ${machine.name}` : ""}</h1>
    <span class="soft">{facts}</span>
    <span class="grow"></span>
    <Action label="Check again" query={setup} />
</div>
{#if setup.error}<Failure error={setup.error} />{/if}

{#if report}
    <section class="card can" aria-labelledby="can">
        <h2 id="can">What this machine can do</h2>
        {#each report.jobs as job (job.name)}
            <div class="row">{@render head(job.state, job.name)}<span class="wide {job.state}">{job.reason}</span></div>
        {/each}
    </section>

    <div class="pair">
        <section class="card" aria-labelledby="tools">
            <h2 id="tools">Tools</h2>
            {#each report.tools as tool (tool.name)}
                <div class="row line">
                    {@render head(tool.state, tool.name)}<span class="wide soft">{tool.needed_for}</span>
                    <code class={tool.state}>{tool.detail}</code>
                </div>
            {/each}
        </section>

        <section class="card" aria-labelledby="credentials">
            <h2 id="credentials">Credentials</h2>
            <p class="soft">Only whether each is set. Values stay in tools/obc.local or your home folder and never show here.</p>
            {#each report.credentials as credential (credential.name)}
                <div class="row line">{@render head(credential.state, credential.name)}<span class="wide soft">{credential.needed_for}</span></div>
            {/each}
        </section>
    </div>

    <div class="pair">
        <section class="card" aria-labelledby="store">
            <h2 id="store">Store</h2>
            <p><code>{report.store.path}</code> · {size(report.store.used_bytes)} used · {free(report.store.free_bytes)}</p>
        </section>

        <section class="card first" aria-labelledby="first">
            <h2 id="first">First steps</h2>
            <ol>
                <li>
                    {#if missing.length}
                        Install {new Intl.ListFormat("en").format(missing)} (the commands are under Tools), then press Check again.
                    {:else}
                        Tools are ready. Ask the owner for an R2 key only if you will publish to prod.
                    {/if}
                </li>
                <li>Open <a href="#/local">local</a>, choose a small area (the Freiburg preset is quick), and start <b>Update local</b>. It takes a few minutes.</li>
                <li>Start <b>Serve</b> and open the web planner or the map builder from there.</li>
                <li>Read the <a href="#/">Overview</a>: sources on the left, layers in the middle, targets on the right.</li>
            </ol>
        </section>
    </div>
{:else if !setup.error}
    <p class="soft">Checking this machine…</p>
{/if}

<style>
    h2 {
        margin-bottom: 6px;
    }
    .card {
        gap: 4px;
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
    .pair > .first {
        flex: 999 1 560px;
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
    .head {
        flex: 0 1 230px;
        min-width: 0;
        display: flex;
        align-items: baseline;
        gap: 12px;
    }
    .can .head {
        flex-basis: 330px;
    }
    .mark {
        flex: none;
        width: 18px;
        font-weight: 600;
    }
    .wide {
        flex: 1 1 140px;
        min-width: 0;
    }
    ol {
        margin: 0;
        padding-left: 22px;
        display: flex;
        flex-direction: column;
        gap: 6px;
    }
    .optional {
        color: var(--ink-faint);
    }
</style>
