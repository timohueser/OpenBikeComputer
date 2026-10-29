<script lang="ts">
    import { boundsOf, nameOf, type Areas } from "../../lib/data/areas";
    import { command } from "../../lib/data/cli";
    import { fill, onlyPrepareReads, preparedBy, RULES, sizeOf, STATUSES, statusNote, whereOf } from "../../lib/data/holding";
    import type { CliQuery } from "../../lib/data/query.svelte";
    import { bakeHash, bakeOf, prepareHash, served } from "../../lib/data/screens";
    import { list, size, when, type Status } from "../../lib/data/shapes";
    import {
        COVERAGE,
        coverage,
        coverageCells,
        groupOf,
        hostsOf,
        isReference,
        modelsOf,
        PARTS,
        placeOf,
        policy,
        referenceOf,
        requestAge,
        requestLabel,
        type Coverage,
        type SourceRow,
    } from "../../lib/data/sources";
    import Action from "./Action.svelte";
    import DataMap from "./DataMap.svelte";

    // One source: how this machine holds it and what fills its gap, its versions or requests, its
    // coverage, its licence, what reads it, and its refresh by target.
    let { row, rows, status, areas }: { row: SourceRow; rows: SourceRow[]; status: CliQuery<Status>; areas: Areas | null } = $props();

    const held = $derived(row.held);
    const gap = $derived(fill(row));
    const reference = $derived(referenceOf(row.id));
    const made = $derived(coverage(row));
    const lock = $derived(row.prepared?.lock);
    const box = (region: string): [number, number, number, number] | null => {
        const b = areas && boundsOf(region, areas);
        return b ? [b.west, b.south, b.east, b.north] : null;
    };
    const cells = $derived(reference ? coverageCells(row, modelsOf(row.id, rows).map((at) => at.row), box) : []);
    const ORDER: Coverage[] = ["lock", "preparing", "archive", "none"];
    const TONE: Record<Coverage, string> = { lock: "missing", preparing: "running", archive: "ready", none: "soft" };
    /** Each version, newest first, with what it covers. */
    const versions = $derived(
        [...(row.prepared?.versions ?? [])]
            .sort((a, b) => b.version.localeCompare(a.version, "en", { numeric: true }))
            .map((version) => ({ ...version, covers: version.region ? nameOf(version.region, areas) : row.extent ? "all of it" : "coverage not recorded" })),
    );
    /** Each request in the store, the oldest first, and whether its refresh is due. */
    const requests = $derived(
        row.requests
            .map((request) => {
                const age = requestAge(request);
                return { request, age, due: typeof row.refresh === "number" && age !== null && age > row.refresh };
            })
            .sort((a, b) => (b.age ?? -1) - (a.age ?? -1)),
    );
    const parts = $derived((row.used_by ?? []).map((part) => PARTS[part] ?? part));
    const readers = $derived([...new Set((status.output?.targets ?? []).flatMap((target) => (target.layers ?? []).filter((layer) => layer.reads.includes(row.id)).map((layer) => layer.layer)))]);
    const by = $derived(onlyPrepareReads(row) ? preparedBy(row) : null);

    let days = $state<Record<string, string>>({});
    /** The policy that `--refresh` takes: whole days or `manual`; `null` for another text. */
    const policyOf = (text = "") => (/^([1-9][0-9]*|manual)$/.test(text.trim()) ? text.trim() : null);
</script>

<a href="#/sources">All sources</a>
<div class="title"><h1>{row.use ?? row.id}</h1></div>
<p class="soft"><code>{row.id}</code> · {groupOf(row)} · from {list(hostsOf(row)) || "this machine"}</p>

<section class="card" aria-labelledby="held">
    <h2 id="held">On this machine</h2>
    <p><span class="pill" data-status={held.status}>{STATUSES[held.status]}</span> {statusNote(held).join(" · ")}</p>
    <p><b>Rule</b> · {RULES[held.rule]}</p>
    <p><b>Covers</b> · {held.covers}</p>
    <p><b>Where</b> · {whereOf(held)}</p>
    <p><b>Size</b> · {sizeOf(held)}{#if held.date}{" · "}<b>Date</b> {held.date}{/if}</p>
    {#if held.archive_files}<p><b>Archive</b> · {held.archive_files.verified} of {held.archive_files.listed} files verified</p>{/if}
    {#each requests as { request, age, due }, i (i)}
        <p>
            {requestLabel(request, (id) => nameOf(id, areas))} · {request.version}{age === null ? "" : ` · ${age} ${age === 1 ? "day" : "days"} old`} · {size(request.bytes)}{#if due}{" · "}<span class="stale">older than its refresh of {policy(row.refresh)}</span>{/if}
        </p>
    {:else}
        {#each row.snapshots as snapshot (snapshot.version)}<p>{snapshot.version} · {size(snapshot.bytes)} in the store</p>{/each}
    {/each}
    {#if gap}
        <div class="warn gap">
            <p>{gap.text}</p>
            {#if gap.prepare}<a class="button" href={prepareHash(gap.prepare)}>Prepare {gap.prepare}…</a>{/if}
            {#if gap.command}<code>{command(gap.command)}</code>{/if}
        </div>
    {/if}
</section>

{#if isReference(row)}
    <section class="card" aria-labelledby="versions">
        <h2 id="versions">Versions</h2>
        <span class={TONE[made]}>{COVERAGE[made].label}</span>
        {#if versions.length}
            <ul class="versions">
                {#each versions as version (version.version)}<li>{version.version} · {size(version.bytes)} · {version.covers}</li>{/each}
            </ul>
        {/if}
        {#if row.prepared?.error}
            <p class="missing">The archive cannot be read: {row.prepared.error}</p>
        {:else if lock?.died}
            <p class="missing">A run on {lock.machine} stopped without freeing the lock. The next preparation takes it over.</p>
        {:else if lock?.stale}
            <p class="missing">No heartbeat from {lock.machine} since {when(lock.heartbeat)}: the run likely stopped.</p>
            <details class="terminal"><summary>In a terminal</summary><code>{command(["unlock", "--source", row.id, "--from", lock.machine])}</code></details>
        {:else if lock}
            <p class="running">On {lock.machine}, at {lock.phase} · <a href="#/runs/{lock.run}">Follow it</a></p>
        {/if}
        {#if made !== "preparing"}<a class="button" href={prepareHash(row.id)}>{versions.length ? (reference?.add ?? "Prepare a new version") : "Prepare"}</a>{/if}
        {#if cells.length}
            <DataMap cells={cells.map((cell) => ({ bounds: [[cell.box[1], cell.box[0]], [cell.box[3], cell.box[2]]], color: COVERAGE[cell.state].color, label: cell.label }))} />
            <ul class="legend" aria-label="Map key">
                {#each ORDER.filter((at) => cells.some((cell) => cell.state === at)) as at (at)}
                    <li><span class="swatch" style:background={COVERAGE[at].color}></span>{at === "none" ? "National model, no version here yet" : COVERAGE[at].label}</li>
                {/each}
            </ul>
        {/if}
    </section>
{/if}

<section class="card" aria-labelledby="licence">
    <h2 id="licence">Licence and credit</h2>
    <p><b>Licence</b> · {row.licence ?? "not recorded"}{#if row.licence_url}{" · "}<a href={row.licence_url}>terms</a>{/if}</p>
    {#if row.attribution}<p><b>Credit</b> · {row.attribution}</p>{/if}
    {#if row.obligations}<p><b>Obligations</b> · {row.obligations}</p>{/if}
    <p>
        <b>Login</b> ·
        {#if !row.credential}none needed{:else if row.credential_missing}<span class="missing">needs {list(row.credential.env ?? [row.credential.file ?? ""])}. <a href="#/machine">Set it on Machine</a></span>{:else}set on this machine{/if}
    </p>
</section>

<section class="card" aria-labelledby="readers">
    <h2 id="readers">Read by</h2>
    {#if by}<p>The prepare of <a href="#/sources/{encodeURIComponent(by)}">{by}</a>; bakes read {by}.</p>{/if}
    {#if parts.length}<p><b>Products</b> · {list(parts)}</p>{/if}
    {#if readers.length}<p><b>Layers</b> · {list(readers)}</p>{/if}
</section>

{#if !isReference(row)}
    <section class="card" aria-labelledby="refresh">
        <h2 id="refresh">Refresh</h2>
        {#each Object.entries(row.targets) as [name, target] (name)}
            <div class="target">
                <b>{placeOf(name, status.output)}</b>
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
                        const at = status.output?.targets.find((at) => at.target === name);
                        location.hash = bakeHash({ ...bakeOf(name, at ? served(at) : []), refresh: `${row.id}=${policyOf(days[name])}` });
                    }}
                >
                    <label>Every {policy(target.refresh)} · new <input bind:value={days[name]} placeholder="30 or manual" /></label>
                    <button type="submit" disabled={!policyOf(days[name])}>Review {placeOf(name, status.output)} with it</button>
                </form>
            </div>
        {/each}
        {#if row.refreshable}
            <Action label="Refresh" args={["source", "refresh", row.id]} folded ondone={(started: { run: string }) => (location.hash = `#/runs/${started.run}`)} />
            <p class="soft">Fetches upstream's newest version. The next bake of each place reads it.</p>
        {/if}
    </section>
{/if}

<style>
    .gap {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 6px;
        padding: 10px 12px;
    }
    .versions {
        margin: 0;
        padding-left: 20px;
        font-variant-numeric: tabular-nums;
    }
    .legend {
        display: flex;
        flex-wrap: wrap;
        gap: 4px 16px;
        margin: 0;
        padding: 0;
        list-style: none;
        font-size: 13px;
    }
    .legend li {
        display: flex;
        align-items: center;
        gap: 6px;
    }
    .swatch {
        width: 14px;
        height: 14px;
        border-radius: 3px;
        opacity: 0.7;
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
        cursor: pointer;
    }
    input {
        width: 9em;
        min-height: 40px;
        margin-left: 6px;
    }
</style>
