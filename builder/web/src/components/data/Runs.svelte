<script lang="ts">
    import { command } from "../../lib/data/cli";
    import type { CliQuery } from "../../lib/data/query.svelte";
    import { clock, elsewhere, MARK, size, span, title, type Holder, type RunList, type Status } from "../../lib/data/shapes";
    import Failure from "./Failure.svelte";
    import Run from "./Run.svelte";
    import RunElsewhere from "./RunElsewhere.svelte";

    // The list of runs of this machine, a run of it, or a run of another machine that holds a lock.
    let { id, runs, status }: { id: string | null; runs: CliQuery<RunList>; status: CliQuery<Status> } = $props();

    const others = $derived(elsewhere(status.output));
    const away = $derived(id ? others.find(({ holder }) => holder.run === id) : undefined);
    // A run of another machine that ends frees its lock; its screen keeps the last lock. A lock that
    // cannot be read says nothing about the run.
    let last = $state<{ target: string; holder: Holder } | undefined>();
    $effect(() => {
        if (away) last = away;
    });
    const watched = $derived(away ?? (last?.holder.run === id ? last : undefined));
    const ended = $derived.by(() => {
        const now = status.output?.targets.find(({ target }) => target === watched?.target);
        return !away && !!now && now.lock === null && now.lock_error === null;
    });
</script>

{#if id && watched}
    <RunElsewhere target={watched.target} holder={watched.holder} {ended} />
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
                            <td class={run.state}>{MARK[run.state]} {run.state}</td>
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
    {#if others.length}
        <h2>On other machines</h2>
        {#each others as { target, holder } (holder.run)}
            <p><a href="#/runs/{holder.run}">{holder.machine} · run {holder.run}</a> changes {target}, now in {holder.phase}.</p>
        {/each}
    {/if}
{/if}

<style>
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
    td:first-child {
        font-family: var(--mono);
        font-size: 13px;
    }
</style>
