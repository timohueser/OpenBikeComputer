<script lang="ts">
    import { boundsOf, nameOf, type Areas } from "../../lib/data/areas";
    import { command } from "../../lib/data/cli";
    import { bytesNote, neededFor, needNote, NEEDS, totals, VERSION_CELLS, versionCells, type RegionSources } from "../../lib/data/holding";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { prepareHash, viewHash } from "../../lib/data/screens";
    import { list } from "../../lib/data/shapes";
    import type { SourceRow } from "../../lib/data/sources";
    import DataMap from "./DataMap.svelte";
    import Failure from "./Failure.svelte";

    // What a bake of one region reads, and what it still downloads or prepares first. The map
    // draws the region over the areas of the prepared versions that it reads.
    let { region, rows, areas }: { region: string | null; rows: SourceRow[]; areas: Areas | null } = $props();

    // svelte-ignore state_referenced_locally
    let picked = $state(region ?? "");
    const query = $derived(region ? new CliQuery<RegionSources>(["sources", "--region", region]) : null);
    $effect(() => void query?.refresh());
    const answer = $derived(query?.output ?? null);
    const held = $derived(answer?.sources.filter((at) => at.for_region.need === "held").length ?? 0);
    const sum = $derived(answer ? totals(answer) : null);
    const cells = $derived(answer && areas ? versionCells(answer, rows, (id) => boundsOf(id, areas)) : []);
</script>

<form class="pick" onsubmit={(event) => (event.preventDefault(), picked.trim() && (location.hash = viewHash("region", picked.trim())))}>
    <label>
        Region
        <input list="region-ids" bind:value={picked} placeholder={areas ? "grimsel, europe/switzerland" : "Reading the areas…"} />
    </label>
    <datalist id="region-ids">
        {#each areas?.regions ?? [] as at (at.id)}<option value={at.id}>{at.name} · saved area</option>{/each}
        {#each (areas?.areas ?? []).filter((at) => !areas?.regions.some((saved) => saved.id === at.id)) as at (at.id)}<option value={at.id}>{at.name}</option>{/each}
    </datalist>
    <button type="submit" class="primary" disabled={!picked.trim()}>Show</button>
    {#if region}<code class="soft">{command(["sources", "--region", region])}</code>{/if}
</form>

{#if query?.error}<Failure error={query.error} />{/if}
{#if !region}
    <p class="soft">Pick a saved area or a Geofabrik area to see what a bake of it reads.</p>
{:else if answer}
    <h2>{answer.name}{answer.countries.length ? ` · ${answer.countries.join(", ")}` : ""}</h2>
    <dl class="counters">
        <div><dt>Sources held</dt><dd>{held} of {answer.sources.length}</dd></div>
        <div><dt>Prepares needed</dt><dd>{answer.prepare.length} <span class="soft">{list(answer.prepare)}</span></dd></div>
        <div>
            <dt>To download once</dt>
            <dd>{bytesNote(sum!.once.bytes, sum!.once.unknown)} {#if sum!.unknownNeed}<span class="soft">{sum!.unknownNeed} {sum!.unknownNeed === 1 ? "source" : "sources"} of unknown need</span>{/if}</dd>
        </div>
        <div><dt>OSM per update</dt><dd>{bytesNote(sum!.osm.bytes, sum!.osm.unknown)}</dd></div>
    </dl>
    {#each answer.products.filter((product) => product.incomplete) as product (product.product)}
        <p class="warn incomplete"><b>{product.product} is incomplete</b>: its step list stops at {product.reason}</p>
    {/each}
    <DataMap
        cells={cells.map((cell) => ({ bounds: [[cell.box.south, cell.box.west], [cell.box.north, cell.box.east]], color: VERSION_CELLS[cell.kind].color, label: cell.label }))}
        outline={answer.boxes.map((box) => [[box.south, box.west], [box.north, box.east]])}
    />
    <ul class="legend" aria-label="Map key">
        <li><span class="swatch outline"></span>The region</li>
        {#each Object.entries(VERSION_CELLS).filter(([kind]) => cells.some((cell) => cell.kind === kind)) as [kind, { color, label }] (kind)}
            <li><span class="swatch" style:background={color}></span>{label}</li>
        {/each}
    </ul>

    <section class="card" aria-labelledby="region-layers">
        <h2 id="region-layers">Layers</h2>
        <div class="table">
            <table>
                <thead><tr><th>Layer</th><th>Need</th><th>Reads</th></tr></thead>
                <tbody>
                    {#each answer.layers as layer (layer.layer)}
                        <tr>
                            <td class="layer"><code>{layer.layer}</code></td>
                            <td><span class="pill" data-need={layer.need}>{NEEDS[layer.need]}</span></td>
                            <td>{layer.reason ?? list(layer.sources)}</td>
                        </tr>
                    {/each}
                </tbody>
            </table>
        </div>
    </section>

    <section class="card" aria-labelledby="region-sources">
        <h2 id="region-sources">Sources</h2>
        <div class="table">
            <table>
                <thead><tr><th>Source</th><th>Held here</th><th>Needed for</th><th>Download</th></tr></thead>
                <tbody>
                    {#each answer.sources as at (at.source)}
                        {@const need = at.for_region.need}
                        {@const reader = at.for_region.read_by ?? at.source}
                        <tr>
                            <td><a href="#/sources/{encodeURIComponent(at.source)}">{at.source}</a></td>
                            <td><span class="pill" data-need={need}>{NEEDS[need]}</span> <span class="soft">{needNote(at)}</span></td>
                            <td>{neededFor(answer, at).join(", ") || "—"}</td>
                            <td>
                                {#if need === "prepare"}
                                    <a href={prepareHash(reader, { flag: "--region", value: region })}>Prepare {reader}…</a>
                                {:else if need === "download"}{bytesNote(at.for_region.download_bytes, at.for_region.unknown_requests)}{:else}—{/if}
                            </td>
                        </tr>
                    {/each}
                </tbody>
            </table>
        </div>
    </section>
{:else if !query?.error}
    <p class="soft">Reading the sources of {nameOf(region, areas)}…</p>
{/if}

<style>
    .pick {
        display: flex;
        flex-wrap: wrap;
        align-items: flex-end;
        gap: 8px 12px;
    }
    .pick label {
        display: flex;
        flex-direction: column;
        gap: 4px;
        font-weight: 650;
    }
    .pick input {
        width: min(320px, 80vw);
        font-weight: 400;
    }
    .counters {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
        gap: 12px;
        margin: 0;
    }
    .counters div {
        display: flex;
        flex-direction: column;
        padding: 12px 16px;
        background: var(--panel);
        border: 1px solid var(--line);
        border-radius: 10px;
    }
    dt {
        color: var(--ink-soft);
        font-size: 12px;
        font-weight: 650;
    }
    dd {
        margin: 0;
        font-size: 22px;
        line-height: 30px;
        font-weight: 700;
        font-variant-numeric: tabular-nums;
    }
    dd .soft {
        display: block;
        font-size: 13px;
        line-height: 18px;
        font-weight: 400;
    }
    .incomplete {
        padding: 10px 14px;
    }
    .swatch.outline {
        border: 2px dashed var(--water);
        opacity: 1;
    }
    table {
        min-width: 640px;
    }
    .layer {
        white-space: nowrap;
    }
    td .soft {
        display: block;
        font-size: 13px;
    }
</style>
