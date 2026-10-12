<script lang="ts">
    import { command } from "../../lib/data/cli";
    import type { CliQuery } from "../../lib/data/query.svelte";
    import { clock, size, span, title, type RunList, type RunState, type Status } from "../../lib/data/shapes";
    import Failure from "./Failure.svelte";
    import Run from "./Run.svelte";
    import RunElsewhere from "./RunElsewhere.svelte";

    // The list of runs of this machine, a run of it, or a run of another machine that holds a lock.
    let { id, runs, status }: { id: string | null; runs: CliQuery<RunList>; status: CliQuery<Status> } = $props();

    const MARK: Record<RunState, string> = {
        starting: "◌ starting",
        running: "◐ running",
        ok: "✓ ok",
        failed: "✗ failed",
        stopped: "■ stopped",
    };
    const here = $derived(status.output?.machine.name);
    const elsewhere = $derived(
        (status.output?.targets ?? []).flatMap(({ target, lock }) => (lock && lock.machine !== here ? [{ target, holder: lock }] : [])),
    );
    const away = $derived(id ? elsewhere.find(({ holder }) => holder.run === id) : undefined);
</script>

{#if id && away}
    <RunElsewhere target={away.target} holder={away.holder} />
{:else if id && (status.output || status.error)}
    {#key id}<Run {id} status={status.output} />{/key}
{:else if !id}
    <h1>Runs</h1>
    <p class="soft">The runs of this machine, newest first. In a terminal: <code>{command(["runs"])}</code></p>
    {#if runs.error}<Failure error={runs.error} />{/if}
    {#if runs.output}
        <div class="table">
            <table>
                <thead>
                    <tr><th>Run</th><th>Command</th><th>State</th><th>Started</th><th>Took</th><th>Fetched</th><th>Built</th></tr>
                </thead>
                <tbody>
                    {#each runs.output.runs as run (run.id)}
                        <tr>
                            <td><a href="#/runs/{run.id}">{run.id}</a></td>
                            <td>{title(run.args)}</td>
                            <td class={run.state}>{MARK[run.state]}</td>
                            <td>{clock(run.started)}</td>
                            <td>{run.wall_ms === null ? "—" : span(run.wall_ms)}</td>
                            <td>{size(run.bytes_fetched)}</td>
                            <td>{size(run.bytes_built)}</td>
                        </tr>
                    {:else}
                        <tr><td colspan="7" class="soft">No run yet.</td></tr>
                    {/each}
                </tbody>
            </table>
        </div>
    {/if}
    {#if elsewhere.length}
        <h2>On other machines</h2>
        {#each elsewhere as { target, holder } (holder.run)}
            <p><a href="#/runs/{holder.run}">{holder.machine} · run {holder.run}</a> changes {target}, now in {holder.phase}.</p>
        {/each}
    {/if}
{/if}

<style>
    h1 {
        font-size: 28px;
        line-height: 36px;
    }
    h2 {
        font-size: 18px;
        line-height: 26px;
    }
    p {
        margin: 0;
    }
    .table {
        overflow-x: auto;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
    }
    table {
        width: 100%;
        border-collapse: collapse;
        white-space: nowrap;
    }
    th,
    td {
        padding: 10px 16px;
        text-align: left;
        border-bottom: 1px solid var(--line);
    }
    th {
        font-weight: 600;
    }
    code,
    td:first-child {
        font-family: var(--mono);
        font-size: 13px;
    }
    .soft {
        color: var(--ink-soft);
    }
    .ok {
        color: var(--ready);
    }
    .starting,
    .running {
        color: var(--running);
    }
    .failed,
    .stopped {
        color: var(--missing);
    }
</style>
