<script lang="ts">
    import { onMount } from "svelte";
    import { boundsOf, nameOf, type Areas } from "../../lib/data/areas";
    import { command } from "../../lib/data/cli";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { prepareHash, sourceAt } from "../../lib/data/screens";
    import { size, when, type Status } from "../../lib/data/shapes";
    import { COVERAGE, coverage, coverageCells, modelsOf, REFERENCES, type Coverage, type SourceRow } from "../../lib/data/sources";
    import DataMap from "./DataMap.svelte";
    import Failure from "./Failure.svelte";
    import Prepare from "./Prepare.svelte";

    // What we make once and keep: each reference as a map of the areas of its versions over its
    // national models, then its versions and the one action that makes the next.
    let { hash, status }: { hash: string; status: CliQuery<Status> } = $props();

    const sources = new CliQuery<{ sources: SourceRow[] }>(["sources"]);
    const areas = new CliQuery<Areas>(["areas"]);
    onMount(() => {
        void sources.refresh();
        void areas.refresh();
    });
    const route = $derived(sourceAt(hash));
    const rows = $derived(sources.output?.sources ?? []);
    const targets = $derived(status.output?.targets ?? []);
    const r2 = (target: string) => (targets.find((at) => at.target === target)?.kind ?? (target === "prod" ? "r2" : "dir")) === "r2";
    const box = (region: string): [number, number, number, number] | null => {
        const b = areas.output && boundsOf(region, areas.output);
        return b ? [b.west, b.south, b.east, b.north] : null;
    };
    const ORDER: Coverage[] = ["lock", "preparing", "r2", "local", "none"];
    const TEXT: Record<Coverage, string> = { lock: "missing", preparing: "running", r2: "ready", local: "stale", none: "soft" };
    /** Each version of `row`, newest first, with where it is and what it covers. */
    const versions = (row: SourceRow) =>
        Object.entries(row.prepared)
            .flatMap(([target, at]) => at.versions.map((version) => ({ target, ...version })))
            .sort((a, b) => b.version.localeCompare(a.version, "en", { numeric: true }))
            .map((version) => ({
                ...version,
                where: r2(version.target) ? "On R2" : "On this computer",
                covers: version.region ? nameOf(version.region, areas.output) : row.extent ? "all of it" : "coverage not recorded",
            }));
</script>

{#if sources.error}<Failure error={sources.error} />{/if}
{#if areas.error}<Failure error={areas.error} />{/if}

{#snippet reference(row: SourceRow, add: string)}
    {@const made = versions(row)}
    {@const state = coverage(row, r2)}
    <div class="member">
        <span class={TEXT[state]}>{COVERAGE[state].label}</span>
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
            <a class="button" href={prepareHash(row.id, "prod")}>{made.length ? add : "Prepare"}</a>
        {/if}
    </div>
{/snippet}

{#if route.prepare && route.id}
    {#key `${route.id}|${route.prepare}`}
        <Prepare id={route.id} {rows} to={route.prepare} area={route.area} {targets} areas={areas.output} />
    {/key}
{:else}
    <h1>References</h1>
    <p class="soft lead">
        A reference is made once from large downloads and kept. Every bake reads only its area from it. On R2, every bake can use it; on this computer, only
        the bakes here.
    </p>
    {#each REFERENCES as { id, title, about, add } (id)}
        {@const row = rows.find((at) => at.id === id)}
        {@const models = modelsOf(id, rows).map((at) => at.row)}
        {@const cells = row ? coverageCells(row, models, r2, box) : []}
        <section class="card" aria-labelledby="reference-{id}">
            <div class="title"><h2 id="reference-{id}">{title}</h2><span class="soft">{about}</span></div>
            {#if cells.length}
                <DataMap cells={cells.map((cell) => ({ bounds: [[cell.box[1], cell.box[0]], [cell.box[3], cell.box[2]]], color: COVERAGE[cell.state].color, label: cell.label }))} />
                <ul class="legend" aria-label="Map key">
                    {#each ORDER.filter((state) => cells.some((cell) => cell.state === state)) as state (state)}
                        <li><span class="swatch" style:background={COVERAGE[state].color}></span>{state === "none" ? "National model, no version here yet" : COVERAGE[state].label}</li>
                    {/each}
                </ul>
            {/if}
            {#if row}{@render reference(row, add)}{:else if sources.output}<p class="soft">Not in data/sources.toml.</p>{/if}
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
    .member {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 4px;
    }
    .versions {
        margin: 0;
        padding-left: 20px;
        font-variant-numeric: tabular-nums;
    }
    summary {
        padding: 8px 0;
        color: var(--ink-soft);
        cursor: pointer;
    }
</style>
