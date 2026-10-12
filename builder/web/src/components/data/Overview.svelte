<script lang="ts">
    import { command } from "../../lib/data/cli";
    import { bands, followers, sourcesOf } from "../../lib/data/graph";
    import type { CliQuery } from "../../lib/data/query.svelte";
    import { age, causes, failure, LAYER_MARK, layerText, list, short, version, when, type Status } from "../../lib/data/shapes";
    import Failure from "./Failure.svelte";

    // The layers of a target as a graph: the sources, then a band for each product with a column
    // for each depth of reads, then the target. The CLI gives the reads of each layer.
    let { status }: { status: CliQuery<Status> } = $props();

    const targets = $derived(status.output?.targets ?? []);
    let chosen = $state<string | null>(null);
    let picked = $state<string | null>(null);
    const target = $derived(targets.find((at) => at.target === (chosen ?? "prod")) ?? targets[0]);
    const layers = $derived(target?.layers ?? []);
    const shown = $derived(bands(layers));
    const named = $derived(new Set(layers.map((layer) => layer.layer)));
    const sources = $derived([...new Set(layers.flatMap((layer) => layer.reads))].filter((name) => !named.has(name)).sort());
    const readers = $derived([...new Set(layers.flatMap((layer) => layer.readers))]);
    const stale = $derived(layers.filter((layer) => layer.state === "stale"));
    const detail = $derived(layers.find((layer) => layer.layer === picked));
    // The layers that the run that holds the lock bakes now.
    const baking = $derived(new Map(target?.lock?.progress.filter((at) => at.done < at.total).map((at) => [at.layer, at]) ?? []));
    const outlined = $derived(new Set(detail ? sourcesOf(layers, detail.layer) : []));
    const only = $derived(detail && target ? ["update", target.target, "--only", detail.layer, "--dry-run"] : []);

    // The arrows: from each layer to each layer that reads it, and from each last layer to the
    // target, measured from the cards. Arrows into one card share the gap left of it: a bus.
    let graph = $state<HTMLElement>();
    let arrows = $state<string[]>([]);
    function measure() {
        if (!graph || !target) return;
        const box = graph.getBoundingClientRect();
        const card = (name: string) => graph!.querySelector(`[data-card="${CSS.escape(name)}"]`)?.getBoundingClientRect();
        const arrow = (from?: DOMRect, to?: DOMRect) => {
            if (!from || !to || from.right > to.left) return [];
            const [y1, y2] = [from.top + from.height / 2 - box.top, to.top + to.height / 2 - box.top];
            return [`M${from.right - box.left} ${y1}H${to.left - box.left - 8}V${y2}H${to.left - box.left - 2}`];
        };
        const reads = layers.flatMap((layer) => layer.reads.filter((read) => named.has(read)).flatMap((read) => arrow(card(read), card(layer.layer))));
        const last = layers.filter((layer) => layer.state !== "off" && !layers.some((other) => other.reads.includes(layer.layer)));
        arrows = [...reads, ...last.flatMap((layer) => arrow(card(layer.layer), card(target!.target)))];
    }
    $effect(() => {
        void [layers, picked];
        const observer = new ResizeObserver(measure);
        if (graph) observer.observe(graph);
        measure();
        return () => observer.disconnect();
    });
</script>

<div class="title">
    <h1>Overview</h1>
    <div class="switch" role="group" aria-label="Target">
        {#each targets as at (at.target)}
            <button
                type="button"
                aria-pressed={at === target}
                onclick={() => {
                    chosen = at.target;
                    picked = null;
                }}>{at.target}</button
            >
        {/each}
    </div>
    <span class="grow"></span>
    {#if target}<a class="primary" href="#/{target.target}/review">Review update of {target.target}</a>{/if}
</div>

{#if target}
    {@const error = failure(target)}
    {#if error}<Failure {error} />{/if}
    <p class="card summary">
        <b>{target.target}</b>
        <span>{target.kind === "r2" ? "R2" : "folder"} · {target.region ?? "no region"} · {target.release ? `release ${short(target.release.serving.id)} since ${when(target.release.serving.since)}` : "serves nothing"}</span>
        {#if stale.length}
            <b class="stale">{stale.length} of {layers.length} layers stale</b>
            <span>Causes: {causes(stale)}</span>
        {:else if layers.length}
            <span class="ok">Each layer is up to date</span>
        {/if}
    </p>

    <div class="graph" bind:this={graph}>
        <svg class="arrows" aria-hidden="true">
            <defs><marker id="head" viewBox="0 0 6 6" refX="5" refY="3" markerWidth="6" markerHeight="6" orient="auto"><path d="M0 0L6 3L0 6z" /></marker></defs>
            {#each arrows as d, i (i)}<path {d} marker-end="url(#head)" />{/each}
        </svg>
        <section class="band" aria-label="Sources">
            <h2>Sources</h2>
            {#each sources as source (source)}
                {@const read = target.sources.filter((at) => at.source === source)}
                <span class="card-node source" class:outlined={outlined.has(source)} data-card={source}>
                    <b>{source}</b>
                    {#each read as { version, retrieved } (version)}<span>{version}{retrieved ? ` · ${age(retrieved)}` : ""}</span>{:else}<span class="soft">not in the release</span>{/each}
                </span>
            {/each}
        </section>
        <div class="products">
            {#each shown as { product, columns } (product)}
                <section class="band" aria-label={product}>
                    <h2>{product}</h2>
                    <div class="columns">
                        {#each columns as column, i (i)}
                            <div class="column">
                                {#each column as layer (layer.layer)}
                                    {@const bakes = baking.get(layer.layer)}
                                    <button
                                        type="button"
                                        class="card-node {bakes ? 'running' : layer.state}"
                                        data-card={layer.layer}
                                        aria-pressed={picked === layer.layer}
                                        onclick={() => (picked = picked === layer.layer ? null : layer.layer)}
                                    >
                                        <b>{layer.layer}</b>
                                        <span>
                                            {bakes ? "◐" : LAYER_MARK[layer.state]}
                                            {[version(layer.format, layer.revision), bakes ? `baking ${bakes.done} of ${bakes.total}` : layerText(layer)]
                                                .filter(Boolean)
                                                .join(" · ")}
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
            <div class="card-node to" data-card={target.target}>
                <b>{target.target}</b>
                <span>{target.at}</span>
                <span><b>Readers</b> · {list(readers)}</span>
                {#if stale.length}<span class="stale">● {stale.length} layers stale</span>{/if}
                <a href="#/{target.target}">Open {target.target}</a>
            </div>
        </section>
    </div>

    <p class="soft">✓ up to date · ● stale: the cause is on the card · ◐ baking · ○ off for this target. Select a layer to see what it reads and what bakes again after it.</p>

    {#if detail}
        <section class="card" aria-labelledby="detail">
            <h2 id="detail">{detail.layer}</h2>
            <p class="soft">{detail.shape === "tiled" ? "Tiled: baked per grid cell" : detail.shape === "whole" ? "Baked once over the coverage" : "Only the release has it"}</p>
            <p>
                Version in the code <b>{version(detail.format, detail.revision) ?? "none"}</b> · in {target.target}
                <b>{version(detail.served_format, detail.served_revision) ?? "none"}</b>
            </p>
            <p class={detail.state}>{LAYER_MARK[detail.state]} {layerText(detail)}</p>
            {#each target.notes.filter((note) => note.layer === detail.layer) as note (note.text)}<p class="missing">{note.text}</p>{/each}
            <p><b>Reads</b> · {list(detail.reads)} (its sources are outlined)</p>
            <p><b>Bakes again after it</b> · {list(followers(layers, detail.layer))}</p>
            <p><b>Used by</b> · {list(detail.readers)}</p>
            <div class="title">
                <a class="primary" href="#/{target.target}/review?only={encodeURIComponent(detail.layer)}">Update only this layer…</a>
                <code>{command(only)}</code>
            </div>
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
        position: relative;
        display: grid;
        grid-template-columns: minmax(140px, 1fr) minmax(0, 4fr) minmax(180px, 1fr);
        gap: 24px;
        padding: 16px;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
    }
    .arrows {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 100%;
        z-index: 1;
        pointer-events: none;
        fill: none;
        stroke: var(--ink-soft);
        stroke-width: 1.5;
    }
    .arrows marker path {
        fill: var(--ink-soft);
        stroke: none;
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
        gap: 24px;
    }
    .column {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    .card-node {
        position: relative;
        z-index: 2;
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
    .card-node b {
        font-size: 14px;
    }
    .card-node.stale {
        border: 2px solid var(--stale);
        background: #fdf3e6;
    }
    .card-node.off {
        border-style: dashed;
        color: var(--ink-faint);
    }
    .card-node.ok span {
        color: var(--ready);
    }
    .card-node.stale span {
        color: var(--stale);
    }
    .card-node.running {
        border: 2px solid var(--running);
    }
    .source span {
        color: var(--ink-soft);
    }
    .outlined,
    .card-node[aria-pressed="true"] {
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
        .arrows {
            display: none;
        }
    }
</style>
