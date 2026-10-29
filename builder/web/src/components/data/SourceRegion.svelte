<script lang="ts">
    import { boundsOf, nameOf, type Areas } from "../../lib/data/areas";
    import { command } from "../../lib/data/cli";
    import { neededFor, NEEDS, osmPerUpdate, STATUSES, type RegionSources } from "../../lib/data/holding";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { prepareHash, viewHash } from "../../lib/data/screens";
    import { list, size } from "../../lib/data/shapes";
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
    const [HELD, GAP] = ["var(--status-archived)", "var(--status-missing)"];
    const held = $derived(answer?.sources.filter((at) => at.for_region.need === "held").length ?? 0);
    const cells = $derived(
        (answer?.sources ?? []).flatMap(({ source, for_region }) =>
            (rows.find((row) => row.id === source)?.prepared?.versions ?? []).flatMap((version) => {
                const box = version.region && areas ? boundsOf(version.region, areas) : null;
                const color = for_region.need === "held" ? HELD : GAP;
                return box ? [{ bounds: [[box.south, box.west], [box.north, box.east]] as [[number, number], [number, number]], color, label: `${source} ${version.version} · ${nameOf(version.region!, areas)}` }] : [];
            }),
        ),
    );
    const downloadOf = (at: RegionSources["sources"][number]) =>
        at.for_region.download_bytes ? size(at.for_region.download_bytes) : at.for_region.unknown_requests ? "size unknown" : "—";
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
        <div><dt>Prepares needed</dt><dd>{answer.prepare.length}</dd>{#if answer.prepare.length}<span class="soft">{list(answer.prepare)}</span>{/if}</div>
        <div>
            <dt>To download once</dt><dd>{size(answer.download_bytes)}</dd>
            {#if answer.unknown.length}<span class="soft">and {answer.unknown.length} of unknown size</span>{/if}
        </div>
        <div><dt>OSM per update</dt><dd>{size(osmPerUpdate(answer))}</dd></div>
    </dl>
    {#each answer.products.filter((product) => product.incomplete) as product (product.product)}
        <p class="warn incomplete"><b>{product.product} is incomplete</b>: its step list stops at {product.reason}</p>
    {/each}
    <DataMap {cells} outline={answer.boxes.map((box) => [[box.south, box.west], [box.north, box.east]])} />
    <ul class="legend" aria-label="Map key">
        <li><span class="swatch outline"></span>The region</li>
        {#if cells.some((cell) => cell.color === HELD)}<li><span class="swatch" style:background={HELD}></span>A prepared version of a source that covers it</li>{/if}
        {#if cells.some((cell) => cell.color === GAP)}<li><span class="swatch" style:background={GAP}></span>A prepared version of a source to prepare</li>{/if}
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
                            <td>{layer.reason ? "Its step list stops, as the note above says." : list(layer.sources)}</td>
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
                <thead><tr><th>Source</th><th>Held here</th><th>Needed for</th><th>Gap</th><th>Download</th></tr></thead>
                <tbody>
                    {#each answer.sources as at (at.source)}
                        {@const need = at.for_region.need}
                        <tr>
                            <td><a href="#/sources/{encodeURIComponent(at.source)}">{at.source}</a></td>
                            <td>
                                <span class="pill" data-status={at.held.status}>{STATUSES[at.held.status]}</span>
                                <span class="soft">
                                    {at.for_region.read_by
                                        ? `through ${at.for_region.read_by}`
                                        : `${at.for_region.held_requests} of ${at.for_region.requests}${at.for_region.held_bytes ? ` · ${size(at.for_region.held_bytes)}` : ""}`}
                                </span>
                            </td>
                            <td>{at.for_region.after_stop ? "after a missing input" : neededFor(answer, at.source).join(", ") || "—"}</td>
                            <td>{#if need === "held"}—{:else}<span class="pill" data-need={need}>{NEEDS[need]}</span>{/if}</td>
                            <td>
                                {#if need === "prepare"}
                                    <a href={prepareHash(at.for_region.read_by ?? at.source, { flag: "--region", value: answer.region })}>Prepare {at.for_region.read_by ?? at.source}…</a>
                                {:else if need === "download"}{downloadOf(at)}{:else}—{/if}
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
    .counters span {
        font-size: 13px;
    }
    .incomplete {
        padding: 10px 14px;
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
