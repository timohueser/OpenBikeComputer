<script lang="ts">
    import type { CliQuery } from "../../lib/data/query.svelte";
    import { LAYER_MARK, short, when, type Layer, type Status } from "../../lib/data/shapes";
    import Failure from "./Failure.svelte";

    // The layers of a target as a graph: the sources, then a band for each product with a column
    // for each depth of reads, then the target. The CLI gives the reads of each layer.
    let { status }: { status: CliQuery<Status> } = $props();

    const targets = $derived(status.output?.targets ?? []);
    let chosen = $state<string | null>(null);
    let picked = $state<string | null>(null);
    const target = $derived(targets.find((at) => at.target === chosen) ?? targets[0]);
    const layers = $derived(target?.layers ?? []);
    const named = $derived(new Map(layers.map((layer) => [layer.layer, layer])));
    const bands = $derived.by(() => {
        const depths = new Map<string, number>();
        const depth = (layer: Layer): number => {
            if (!depths.has(layer.layer)) {
                depths.set(layer.layer, 0);
                const reads = layer.reads.flatMap((name) => named.get(name) ?? []);
                depths.set(layer.layer, Math.max(-1, ...reads.map(depth)) + 1);
            }
            return depths.get(layer.layer)!;
        };
        const bands = new Map<string, Layer[][]>();
        for (const layer of layers) {
            const columns = bands.get(layer.layer.split("/")[0]) ?? [];
            (columns[depth(layer)] ??= []).push(layer);
            bands.set(layer.layer.split("/")[0], columns);
        }
        return [...bands].map(([product, columns]) => ({ product, columns: columns.filter(Boolean) }));
    });
    const sources = $derived([...new Set(layers.flatMap((layer) => layer.reads))].filter((name) => !named.has(name)).sort());
    const readers = $derived([...new Set(layers.flatMap((layer) => layer.readers))].filter((name) => !named.has(name)));
    const stale = $derived(layers.filter((layer) => layer.state === "stale"));
    const detail = $derived(picked ? named.get(picked) : undefined);
    const list = (items: string[]) => (items.length ? new Intl.ListFormat("en").format(items) : "nothing");
</script>

<div class="title">
    <h1>Overview</h1>
    <div class="switch" role="group" aria-label="Target">
        {#each targets as at (at.target)}
            <button type="button" aria-pressed={at === target} onclick={() => ((chosen = at.target), (picked = null))}>{at.target}</button>
        {/each}
    </div>
    <span class="grow"></span>
    {#if target}<a class="primary" href="#/{target.target}/review">Review update of {target.target}</a>{/if}
</div>

{#if target}
    {#if target.error}<Failure error={{ code: "failed", message: target.error, fix: target.fix ?? "" }} />{/if}
    <p class="card summary">
        <b>{target.target}</b>
        <span>{target.kind === "r2" ? "R2" : "folder"} · {target.region ?? "no region"} · {target.release ? `release ${short(target.release.serving.id)} since ${when(target.release.serving.since)}` : "serves nothing"}</span>
        {#if stale.length}
            <b class="stale">{stale.length} of {layers.length} layers stale</b>
            <span>Causes: {[...new Set(stale.map((layer) => layer.text))].join(" · ")}</span>
        {:else if layers.length}
            <span class="ok">Each layer is up to date</span>
        {/if}
    </p>

    <div class="graph">
        <section class="band sources" aria-label="Sources">
            <h2>Sources</h2>
            {#each sources as source (source)}<span class="node">{source}</span>{/each}
        </section>
        <div class="products">
            {#each bands as { product, columns } (product)}
                <section class="band" aria-label={product}>
                    <h2>{product}</h2>
                    <div class="columns">
                        {#each columns as column, i (i)}
                            <div class="column">
                                {#each column as layer (layer.layer)}
                                    <button
                                        type="button"
                                        class="node {layer.state}"
                                        aria-pressed={picked === layer.layer}
                                        onclick={() => (picked = picked === layer.layer ? null : layer.layer)}
                                    >
                                        <b>{layer.layer}</b>
                                        <span>
                                            {LAYER_MARK[layer.state]}
                                            {layer.format === null ? "" : `${layer.format}.${layer.revision} · `}{layer.text ?? (layer.state === "off" ? "off" : "up to date")}
                                        </span>
                                    </button>
                                {/each}
                            </div>
                        {/each}
                    </div>
                </section>
            {/each}
        </div>
        <section class="band" aria-label="Target">
            <h2>Target</h2>
            <div class="node to">
                <b>{target.target}</b>
                <span>{target.at}</span>
                <span><b>Readers</b> · {list(readers)}</span>
                {#if stale.length}<span class="stale">● {stale.length} layers stale</span>{/if}
                <a href="#/{target.target}">Open {target.target}</a>
            </div>
        </section>
    </div>

    <p class="soft">✓ up to date · ● stale: the cause is on the box · ○ off for this target. Select a layer to see what it reads and what reads it.</p>

    {#if detail}
        <section class="card detail" aria-labelledby="detail">
            <h2 id="detail">{detail.layer}</h2>
            <p class="soft">{detail.shape === "tiled" ? "Tiled: baked per grid cell" : detail.shape === "whole" ? "Baked once over the coverage" : "Only the release has it"}</p>
            {#if detail.format !== null}<p>Format {detail.format}, revision {detail.revision}</p>{/if}
            <p class={detail.state}>{LAYER_MARK[detail.state]} {detail.text ?? (detail.state === "off" ? "Off" : "Up to date")}</p>
            <p><b>Reads</b> · {list(detail.reads)}</p>
            <p><b>Read by the layers</b> · {list(layers.filter((layer) => layer.reads.includes(detail.layer)).map((layer) => layer.layer))}</p>
            <p><b>Used by</b> · {list(detail.readers)}</p>
        </section>
    {/if}
{:else if !status.error}
    <p class="soft">Reading the targets…</p>
{/if}

<style>
    .switch {
        display: flex;
        border: 1px solid var(--ink);
        border-radius: 8px;
        overflow: hidden;
    }
    .switch button {
        border: 0;
        border-radius: 0;
    }
    .switch button[aria-pressed="true"] {
        background: var(--ink);
        color: var(--panel);
    }
    .summary {
        flex-direction: row;
        flex-wrap: wrap;
        gap: 4px 16px;
        padding: 12px 16px;
    }
    .graph {
        display: grid;
        grid-template-columns: minmax(140px, 1fr) minmax(0, 4fr) minmax(180px, 1fr);
        gap: 16px;
        padding: 16px;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
    }
    .products {
        display: flex;
        flex-direction: column;
        gap: 16px;
        min-width: 0;
    }
    .band {
        display: flex;
        flex-direction: column;
        gap: 8px;
        min-width: 0;
        padding: 12px;
        background: var(--parchment);
        border-radius: 8px;
    }
    .band h2 {
        font-size: 13px;
        line-height: 18px;
        letter-spacing: 0.08em;
        text-transform: uppercase;
    }
    .columns {
        display: grid;
        grid-auto-flow: column;
        grid-auto-columns: minmax(0, 1fr);
        gap: 12px;
    }
    .column {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    .node {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 2px;
        min-height: 0;
        padding: 8px 10px;
        background: var(--panel);
        color: var(--ink);
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        font: 400 13px/18px var(--sans);
        text-align: left;
        overflow-wrap: anywhere;
    }
    .node b {
        font-size: 14px;
    }
    .node.stale {
        border: 2px solid var(--stale);
        background: #fdf3e6;
    }
    .node.off {
        border-style: dashed;
        color: var(--ink-faint);
    }
    .node.ok span {
        color: var(--ready);
    }
    .node.stale span {
        color: var(--stale);
    }
    .node[aria-pressed="true"] {
        outline: 3px solid var(--ink);
        outline-offset: 2px;
    }
    .to {
        border: 2px solid var(--ink);
        gap: 8px;
    }
    @media (max-width: 800px) {
        .graph {
            grid-template-columns: minmax(0, 1fr);
        }
        .columns {
            grid-auto-flow: row;
        }
    }
</style>
