<script lang="ts">
    import { onMount } from "svelte";
    import { nameOf, type Areas } from "../../lib/data/areas";
    import { command } from "../../lib/data/cli";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { sourceAt } from "../../lib/data/screens";
    import { list, size, type Status } from "../../lib/data/shapes";
    import { daysOld, GROUPS, hostsOf, isReference, PARTS, policy, requestAge, requestLabel, requestsLine, type SourceRow } from "../../lib/data/sources";
    import Action from "./Action.svelte";
    import Failure from "./Failure.svelte";

    // What bakes fetch from the internet, by kind, and one source with its licence and refresh.
    let { hash, status }: { hash: string; status: CliQuery<Status> } = $props();

    const sources = new CliQuery<{ sources: SourceRow[] }>(["sources"]);
    const areas = new CliQuery<Areas>(["areas"]);
    onMount(() => {
        void sources.refresh();
        void areas.refresh();
    });
    const rows = $derived((sources.output?.sources ?? []).filter((row) => !isReference(row)));
    const groups = $derived([...GROUPS, ...new Set(rows.map((row) => row.group ?? "Other").filter((group) => !GROUPS.includes(group)))]);
    const row = $derived(rows.find((row) => row.id === sourceAt(hash).id));
    const where = (target: string) => (status.output?.targets.find((at) => at.target === target)?.kind ?? (target === "prod" ? "r2" : "dir")) === "r2" ? "Production" : "This computer";

    /** What a person acts on: a release that reads an old or blocked version, by where it is. */
    const due = (row: SourceRow) =>
        Object.entries(row.targets).flatMap(([target, at]) =>
            at.requests.filter((request) => request.state === "stale" || request.state === "blocked").map((request) => `Due again for ${where(target).toLowerCase()}: ${request.reason ?? request.state}`),
        );
    const needsAction = (row: SourceRow) => row.credential_missing || due(row).length > 0;
    function here(row: SourceRow): string | null {
        const total = row.snapshots.reduce((sum, snapshot) => sum + snapshot.bytes, 0);
        if (row.requests.length) return requestsLine(row.requests, total);
        const newest = row.snapshots[0];
        if (!newest) return null;
        const days = daysOld(newest.version);
        const versions = row.snapshots.length > 1 ? ` in ${row.snapshots.length} versions` : "";
        return `${newest.version}${days === null ? "" : ` · ${days} ${days === 1 ? "day" : "days"} old`} · ${size(total)}${versions}`;
    }
    /** Each request of `row` in the store, the oldest first, and whether its refresh is due. */
    const requests = (row: SourceRow) =>
        row.requests
            .map((request) => {
                const age = requestAge(request);
                return { request, age, due: typeof row.refresh === "number" && age !== null && age > row.refresh };
            })
            .sort((a, b) => (b.age ?? -1) - (a.age ?? -1));
    const parts = (row: SourceRow) => (row.used_by ?? []).map((part) => PARTS[part] ?? part);

    let days = $state<Record<string, string>>({});
    /** The policy that `--refresh` takes: whole days or `manual`; `null` for another text. */
    const policyOf = (text = "") => (/^([1-9][0-9]*|manual)$/.test(text.trim()) ? text.trim() : null);
</script>

{#if sources.error}<Failure error={sources.error} />{/if}

{#snippet login(row: SourceRow)}
    <span class="missing">Needs a login: {list(row.credential?.env ?? [row.credential?.file ?? ""])}. <a href="#/machine">Set it on Machine</a></span>
{/snippet}

{#if row}
    <a href="#/downloads">All downloads</a>
    <div class="title">
        <h1>{row.use ?? row.id}</h1>
    </div>
    <p class="soft">{row.group ?? "Other"} · from {list(hostsOf(row)) || "this machine"}</p>
    <section class="card" aria-labelledby="here">
        <h2 id="here">On this machine</h2>
        {#each requests(row) as { request, age, due }, i (i)}
            <p>
                {requestLabel(request, (id) => nameOf(id, areas.output))} · {request.version}{age === null ? "" : ` · ${age} ${age === 1 ? "day" : "days"} old`} · {size(request.bytes)}{#if due}{" · "}<span class="stale">older than its refresh of {policy(row.refresh)}</span>{/if}
            </p>
        {:else}
            {#each row.snapshots as snapshot (snapshot.version)}
                <p>{snapshot.version} · {size(snapshot.bytes)}</p>
            {:else}
                <p class="soft">Not downloaded yet. A bake that needs it fetches it.</p>
            {/each}
        {/each}
        {#if parts(row).length}<p><b>Used by</b> · {list(parts(row))}</p>{/if}
    </section>
    <section class="card" aria-labelledby="licence">
        <h2 id="licence">Licence and access</h2>
        <p>
            <b>Licence</b> · {row.licence ?? "not recorded"}{#if row.licence_url}
                · <a href={row.licence_url}>terms</a>{/if}
        </p>
        {#if row.attribution}<p><b>Credit</b> · {row.attribution}</p>{/if}
        {#if row.obligations}<p><b>Obligations</b> · {row.obligations}</p>{/if}
        <p><b>Login</b> · {#if !row.credential}none needed{:else if row.credential_missing}{@render login(row)}{:else}set on this machine{/if}</p>
    </section>
    <section class="card" aria-labelledby="refresh">
        <h2 id="refresh">Refresh</h2>
        {#each Object.entries(row.targets) as [name, target] (name)}
            <div class="target">
                <b>{where(name)}</b>
                {#each target.requests as request, i (i)}
                    <span class={request.state === "ok" ? "" : request.state}>
                        reads {request.serving ?? "nothing"}{request.observation.result.value ? ` · upstream ${request.observation.result.value}` : ""}{request.age_days !== null
                            ? ` · ${request.age_days} days old`
                            : ""}{request.reason ? ` · ${request.reason}` : ""}
                    </span>
                {:else}
                    <span class="soft">Its release reads none of it.</span>
                {/each}
                <form
                    class="title"
                    onsubmit={(event) => {
                        event.preventDefault();
                        location.hash = `#/${name}/review?refresh=${encodeURIComponent(`${row.id}=${policyOf(days[name])}`)}`;
                    }}
                >
                    <label>Every {policy(target.refresh)} · new <input bind:value={days[name]} placeholder="30 or manual" /></label>
                    <button type="submit" disabled={!policyOf(days[name])}>Review {where(name)} with it</button>
                </form>
            </div>
        {/each}
        {#if row.refreshable}
            <Action label="Refresh" args={["source", "refresh", row.id]} folded ondone={(started: { run: string }) => (location.hash = `#/runs/${started.run}`)} />
            <p class="soft">Fetches upstream's newest version. The next bake of each place reads it.</p>
        {/if}
    </section>
    <details class="terminal"><summary>In a terminal</summary><code>{command(["sources"])}</code> lists it as <code>{row.id}</code>.</details>
{:else}
    <h1>Downloads</h1>
    <p class="soft">What bakes fetch from the internet. A bake fetches what it needs and keeps it in the store of this machine.</p>
    {#each groups as group (group)}
        {@const inGroup = rows.filter((row) => (row.group ?? "Other") === group).sort((a, b) => Number(needsAction(b)) - Number(needsAction(a)))}
        {#if inGroup.length}
            <details class="card" open>
                <summary><h2>{group}</h2></summary>
                <ul class="rows">
                    {#each inGroup as row (row.id)}
                        {@const at = here(row)}
                        <li>
                            <span class="what">
                                <a href="#/downloads/{encodeURIComponent(row.id)}">{row.use ?? row.id}</a>
                                <span class="soft">{list(hostsOf(row)) || "installed on this machine"}</span>
                            </span>
                            <span class:soft={!at}>{at ?? "Not downloaded yet"}</span>
                            <span class="parts">{#each parts(row) as part (part)}<span class="part">{part}</span>{/each}</span>
                            {#if row.credential_missing}{@render login(row)}{/if}
                            {#each due(row) as reason (reason)}<span class="stale">{reason}</span>{/each}
                        </li>
                    {/each}
                </ul>
            </details>
        {/if}
    {/each}
{/if}

<style>
    summary {
        cursor: pointer;
    }
    summary h2 {
        display: inline;
    }
    .rows {
        display: flex;
        flex-direction: column;
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .rows li {
        display: grid;
        grid-template-columns: minmax(0, 2fr) minmax(0, 1fr) minmax(0, 1fr);
        gap: 4px 16px;
        padding: 10px 0;
        border-bottom: 1px solid var(--line);
    }
    .rows li:last-child {
        border-bottom: 0;
    }
    .rows li > :is(.missing, .stale) {
        grid-column: 1 / -1;
    }
    .what {
        display: flex;
        flex-direction: column;
    }
    .what a {
        color: var(--ink);
        font-weight: 600;
    }
    .what .soft {
        font-size: 13px;
        line-height: 19px;
    }
    .parts {
        display: flex;
        flex-wrap: wrap;
        align-content: flex-start;
        gap: 4px;
    }
    .part {
        padding: 0 8px;
        border-radius: 999px;
        background: var(--parchment-3);
        font-size: 12px;
        line-height: 20px;
        font-weight: 600;
    }
    .target {
        display: flex;
        flex-direction: column;
        gap: 2px;
        padding: 8px 0;
        border-bottom: 1px solid var(--line);
    }
    .terminal summary {
        color: var(--ink-soft);
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
    @media (max-width: 760px) {
        .rows li {
            grid-template-columns: 1fr;
        }
    }
</style>
