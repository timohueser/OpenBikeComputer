<script lang="ts">
    import { onMount } from "svelte";
    import { boundsOf, nameOf, type Areas } from "../../lib/data/areas";
    import { command } from "../../lib/data/cli";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { prepareHash, sourceAt } from "../../lib/data/screens";
    import { size, when, type Status } from "../../lib/data/shapes";
    import { COVERAGE, coverage, coverageCells, family, FAMILIES, isReference, type Coverage, type SourceRow } from "../../lib/data/sources";
    import DataMap from "./DataMap.svelte";
    import Failure from "./Failure.svelte";
    import Prepare from "./Prepare.svelte";

    // What we make once and keep: each family as a coverage map of its members, then the members,
    // each with its versions and the one action that makes the next.
    let { hash, status }: { hash: string; status: CliQuery<Status> } = $props();

    const sources = new CliQuery<{ sources: SourceRow[] }>(["sources"]);
    const areas = new CliQuery<Areas>(["areas"]);
    onMount(() => {
        void sources.refresh();
        void areas.refresh();
    });
    const route = $derived(sourceAt(hash));
    const rows = $derived((sources.output?.sources ?? []).filter(isReference));
    const targets = $derived(status.output?.targets ?? []);
    const r2 = (target: string) => (targets.find((at) => at.target === target)?.kind ?? (target === "prod" ? "r2" : "dir")) === "r2";
    const box = (region: string): [number, number, number, number] | null => {
        const b = areas.output && boundsOf(region, areas.output);
        return b ? [b.west, b.south, b.east, b.north] : null;
    };
    const ORDER: Coverage[] = ["lock", "preparing", "r2", "local", "none"];
    const TEXT: Record<Coverage, string> = { lock: "missing", preparing: "running", r2: "ready", local: "stale", none: "soft" };
    const members = (name: string) =>
        rows
            .filter((row) => family(row.id) === name)
            .map((row) => ({ row, state: coverage(row, r2) }))
            .sort((a, b) => ORDER.indexOf(a.state) - ORDER.indexOf(b.state) || (a.row.use ?? a.row.id).localeCompare(b.row.use ?? b.row.id));
    /** Each version of `row`, newest first, with where it is and what it covers. */
    const versions = (row: SourceRow) =>
        Object.entries(row.prepared)
            .flatMap(([target, at]) => at.versions.map((version) => ({ target, ...version })))
            .sort((a, b) => b.version.localeCompare(a.version))
            .map((version) => ({
                ...version,
                where: r2(version.target) ? "On R2" : "On this computer",
                covers: version.region ? nameOf(version.region, areas.output) : row.extent ? "all of it" : "coverage not recorded",
            }));
</script>

{#if sources.error}<Failure error={sources.error} />{/if}
{#if areas.error}<Failure error={areas.error} />{/if}

{#snippet member({ row, state }: { row: SourceRow; state: Coverage })}
    {@const made = versions(row)}
    <li>
        <span class="what"><b>{row.use ?? row.id}</b><span class={TEXT[state]}>{COVERAGE[state].label}</span></span>
        {#if made.length}
            <ul class="versions">
                {#each made as version (`${version.target}|${version.version}`)}
                    <li>{version.version} · {size(version.bytes)} · {version.where} · {version.covers}</li>
                {/each}
            </ul>
        {/if}
        {#each Object.entries(row.prepared) as [target, at] (target)}
            {#if at.error}
                <span class="missing">{r2(target) ? "R2" : "This computer"} cannot be read: {at.error}</span>
            {:else if at.lock?.died}
                <span class="missing">A run on {at.lock.machine} stopped without freeing the lock. The next preparation takes it over.</span>
            {:else if at.lock?.stale}
                <span class="missing">No heartbeat from {at.lock.machine} since {when(at.lock.heartbeat)}: the run likely stopped.</span>
                <details class="terminal">
                    <summary>In a terminal</summary>
                    <code>{command(["unlock", target, "--source", row.id, "--from", at.lock.machine])}</code>
                </details>
            {:else if at.lock}
                <span class="running">On {at.lock.machine}, at {at.lock.phase} · <a href="#/runs/{at.lock.run}">Follow it</a></span>
            {/if}
        {/each}
        {#if state !== "preparing"}
            <a class="act" href={prepareHash(row.id, "prod")}>{made.length ? (FAMILIES.find((at) => at.family === family(row.id))?.add ?? "Prepare") : "Prepare"}</a>
        {/if}
    </li>
{/snippet}

{#if route.prepare && route.id}
    {#key `${route.id}|${route.prepare}`}
        <Prepare id={route.id} row={rows.find((row) => row.id === route.id) ?? null} to={route.prepare} area={route.area} {targets} areas={areas.output} />
    {/key}
{:else}
    <h1>References</h1>
    <p class="soft lead">
        A reference is made once from large downloads and kept. Every bake reads only its area from it. On R2, every bake can use it; on this computer, only
        the bakes here.
    </p>
    {#each FAMILIES as { family: name, title, about } (name)}
        {@const all = members(name)}
        {@const cells = coverageCells(all.map(({ row }) => row), r2, box)}
        {@const shown = all.filter(({ state }) => state !== "none")}
        {@const rest = all.filter(({ state }) => state === "none")}
        <section class="card" aria-labelledby="family-{name}">
            <div class="title"><h2 id="family-{name}">{title}</h2><span class="soft">{about}</span></div>
            {#if cells.length}
                <DataMap cells={cells.map((cell) => ({ bounds: [[cell.box[1], cell.box[0]], [cell.box[3], cell.box[2]]], color: COVERAGE[cell.state].color, label: cell.label }))} />
            {:else if all.length}
                <p class="soft">No version records its area, so the map has nothing to draw.</p>
            {/if}
            <ul class="legend" aria-label="Map key">
                {#each ORDER.filter((state) => all.some((at) => at.state === state)) as state (state)}
                    <li><span class="swatch" style:background={COVERAGE[state].color}></span>{COVERAGE[state].label} · {all.filter((at) => at.state === state).length}</li>
                {/each}
            </ul>
            {#if sources.output && !all.length}<p class="soft">No source of this family.</p>{/if}
            {#if shown.length}
                <ul class="members">
                    {#each shown as at (at.row.id)}{@render member(at)}{/each}
                </ul>
            {/if}
            {#if rest.length}
                <details open={!shown.length}>
                    <summary>Not prepared yet · {rest.length}</summary>
                    <ul class="members">
                        {#each rest as at (at.row.id)}{@render member(at)}{/each}
                    </ul>
                </details>
            {/if}
        </section>
    {/each}
{/if}

<style>
    .lead {
        max-width: 72ch;
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
    .members {
        display: flex;
        flex-direction: column;
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .members > li {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 4px;
        padding: 12px 0;
        border-bottom: 1px solid var(--line);
    }
    .what {
        display: flex;
        flex-wrap: wrap;
        gap: 4px 12px;
    }
    .versions {
        margin: 0;
        padding-left: 20px;
        font-variant-numeric: tabular-nums;
    }
    .act {
        display: inline-flex;
        align-items: center;
        min-height: 40px;
        margin-top: 4px;
        padding: 0 16px;
        border: 1px solid var(--ink);
        border-radius: 8px;
        color: var(--ink);
        font-weight: 600;
        text-decoration: none;
    }
    .act:hover {
        background: var(--parchment);
    }
    .act:focus-visible {
        outline: 2px solid var(--running);
        outline-offset: 2px;
    }
    summary {
        padding: 8px 0;
        color: var(--ink-soft);
        cursor: pointer;
    }
</style>
