<script lang="ts">
    import { onMount } from "svelte";
    import { command } from "../../lib/data/cli";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { sourceAt } from "../../lib/data/screens";
    import { list, type Status } from "../../lib/data/shapes";
    import { family, ORDER, policy, worst, type SourceRow } from "../../lib/data/sources";
    import Action from "./Action.svelte";
    import Failure from "./Failure.svelte";
    import Prepare from "./Prepare.svelte";

    // The sources of `data/sources.toml` by group, and one source with what each target reads of it.
    let { hash, status }: { hash: string; status: CliQuery<Status> } = $props();

    const sources = new CliQuery<{ sources: SourceRow[] }>(["sources"]);
    onMount(() => void sources.refresh());
    const route = $derived(sourceAt(hash));
    const rows = $derived(sources.output?.sources ?? []);
    const targets = $derived(status.output?.targets ?? []);
    // The rows of a prepared family, such as `terrain-reference/ch`, show as the family.
    const shown = $derived(rows.filter((row, i) => !row.id.includes("/") || rows.findIndex((at) => family(at.id) === family(row.id)) === i));
    const groups = $derived([...new Set(shown.map((row) => row.group ?? "Other"))]);
    const members = (id: string) => rows.filter((row) => row.id === id || family(row.id) === id);
    const stateOf = (id: string) => worst(members(id).flatMap((row) => Object.values(row.targets).map((target) => target.state)));
    const usedBy = (id: string) => targets.flatMap((target) => (target.layers ?? []).filter((layer) => layer.reads.includes(id)).map((layer) => layer.layer));
    const staleBy = (id: string) =>
        targets.flatMap((target) =>
            (target.layers ?? []).filter((layer) => layer.cause?.source === id).map((layer) => `${target.target} ${layer.layer}`),
        );
    const versions = (row: SourceRow) =>
        Object.entries(row.targets).flatMap(([name, target]) => target.requests.map((request) => `${name} ${request.serving ?? "—"}`));
    const id = $derived(route.id && (members(route.id).length ? route.id : null));
    const row = $derived(rows.find((row) => row.id === id));
    let days = $state<Record<string, string>>({});
    // A refresh fetches upstream's newest version of a `digest` source: a detached run.
    const refreshable = $derived(row?.version === "digest" && row.fetch.kind !== "prepared");
</script>

{#if sources.error}<Failure error={sources.error} />{/if}

{#if route.prepare && id}
    <Prepare {id} to={route.prepare} kind={targets.find((target) => target.target === route.prepare)?.kind ?? "dir"} />
{:else if id}
    <div class="title">
        <h1>{id}</h1>
        <span class="soft">{row ? `${row.kind} · ${row.group ?? "Other"}` : `${members(id).length} prepared sources`}</span>
        <span class={stateOf(id)}>{stateOf(id)}</span>
    </div>
    <a href="#/sources">All sources</a>
    {#if row}
        <section class="card" aria-labelledby="licence">
            <h2 id="licence">Licence and access</h2>
            <p>
                <b>Licence</b> · {row.licence ?? "not recorded"}{#if row.licence_url}
                    · <a href={row.licence_url}>terms</a>{/if}
            </p>
            {#if row.attribution}<p><b>Credit</b> · {row.attribution}</p>{/if}
            {#if row.obligations}<p><b>Obligations</b> · {row.obligations}</p>{/if}
            <p>
                <b>Credential</b> · {#if !row.credential}none needed{:else if row.credential_missing}<span class="missing"
                        >missing on this machine: set {list(row.credential.env ?? [row.credential.file ?? ""])}</span
                    >{:else}set on this machine{/if}
            </p>
        </section>
        <section class="card" aria-labelledby="targets">
            <h2 id="targets">On each target</h2>
            {#each Object.entries(row.targets) as [name, target] (name)}
                <div class="target">
                    <b>{name}</b>
                    <span class={target.state}>{target.state}</span>
                    {#each target.requests as request, i (i)}
                        <span>
                            reads {request.serving ?? "nothing"}{request.observation.result.value ? ` · upstream ${request.observation.result.value}` : ""}{request.age_days !== null
                                ? ` · ${request.age_days} days old`
                                : ""}{request.reason ? ` · ${request.reason}` : ""}
                        </span>
                    {/each}
                    <form
                        class="title"
                        onsubmit={(event) => {
                            event.preventDefault();
                            location.hash = `#/${name}/review?refresh=${encodeURIComponent(`${row.id}=${days[name]}`)}`;
                        }}
                    >
                        <label>Refresh: {policy(target.refresh)} · new <input bind:value={days[name]} placeholder="30 or manual" required /></label>
                        <button type="submit">Review {name} with it</button>
                    </form>
                </div>
            {/each}
            <p><b>Used by</b> · {list(usedBy(row.id))}</p>
            <p><b>Makes stale</b> · {list(staleBy(row.id))}</p>
            {#if refreshable}
                <Action label="Refresh" args={["source", "refresh", row.id]} ondone={(started: { run: string }) => (location.hash = `#/runs/${started.run}`)} />
                <p class="soft">A run fetches upstream's newest version; the next update of each target that serves the version of now reads it.</p>
            {/if}
        </section>
    {/if}
    {#if members(id).some((member) => member.fetch.kind === "prepared")}
        <Prepare {id} rows={members(id)} />
    {/if}
{:else}
    <h1>Sources</h1>
    <p class="soft">Each external dataset, asset and tool of data/sources.toml. In a terminal: <code>{command(["sources"])}</code></p>
    {#each groups as group (group)}
        {@const inGroup = shown
            .filter((row) => (row.group ?? "Other") === group)
            .sort((a, b) => ORDER.indexOf(stateOf(family(a.id))) - ORDER.indexOf(stateOf(family(b.id))))}
        <details class="card" open={inGroup.some((row) => ["blocked", "stale"].includes(stateOf(family(row.id))))}>
            <summary><h2>{group}</h2> <span class={worst(inGroup.map((row) => stateOf(family(row.id))))}>{worst(inGroup.map((row) => stateOf(family(row.id))))}</span></summary>
            <div class="table">
                <table>
                    <thead><tr><th>Source</th><th>Version</th><th>Refresh</th><th>Used by</th><th>State</th></tr></thead>
                    <tbody>
                        {#each inGroup as row (row.id)}
                            {@const id = row.id.includes("/") ? family(row.id) : row.id}
                            <tr>
                                <td><a href="#/sources/{encodeURIComponent(id)}">{id}</a></td>
                                <td>{row.id.includes("/") ? `${members(id).length} prepared` : versions(row).join(", ")}</td>
                                <td>{Object.entries(row.targets).map(([name, target]) => `${name} ${policy(target.refresh)}`).join(", ")}</td>
                                <td>{usedBy(id).length} layers</td>
                                <td class={stateOf(id)}>{stateOf(id)}</td>
                            </tr>
                        {/each}
                    </tbody>
                </table>
            </div>
        </details>
    {/each}
{/if}

<style>
    summary {
        display: flex;
        gap: 12px;
        align-items: baseline;
        cursor: pointer;
    }
    .target {
        display: flex;
        flex-direction: column;
        gap: 2px;
        padding: 8px 0;
        border-bottom: 1px solid var(--line);
    }
    .table {
        overflow-x: auto;
    }
    table {
        width: 100%;
        border-collapse: collapse;
        white-space: nowrap;
    }
    th,
    td {
        padding: 8px 12px;
        text-align: left;
        border-bottom: 1px solid var(--line);
    }
    input {
        width: 9em;
        min-height: 40px;
        margin-left: 6px;
        padding: 0 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        font: inherit;
    }
</style>
