<script lang="ts">
    import {
        addLodTier,
        autoSimplify,
        editLodTier,
        removeLodTier,
        setLodCoverageSimplify,
    } from "../../lib/config/edit";
    import { working } from "../../lib/config/storage.svelte";

    const env = $derived(working.envelope!);
    const lods = $derived(env.config.lods);

    function edit(i: number, field: "max_mpp" | "simplify" | "min_area_px" | "min_line_km", raw: string) {
        const v = parseFloat(raw);
        editLodTier(env.config, i, field, Number.isFinite(v) ? v : 0);
        working.markModified();
    }
</script>

<div class="card">
    <p class="muted small intro">
        Each tier is a zoom band: LOD 0 is the coarsest overview, the last tier the finest.
        <em>max m/px</em> is the most zoomed-out scale a tier covers; <em>simplify</em> drops
        geometry detail finer than that many meters. A feature appears from its start tier
        (the “levels” control in Features &amp; styling) and every finer one.
    </p>
    <p class="muted small intro">
        Simplify defaults to the next finer tier's <em>max m/px</em> — geometry stays accurate to
        one pixel at every scale the tier is drawn at — and follows that ceiling until you type
        your own value.
    </p>
    <p class="muted small intro">
        <em>min area</em> drops <strong>area features</strong> (forests, landuse, water) whose
        on-screen area would be smaller than that many pixels² at this tier — a coarse-view
        declutter that keeps sub-pixel slivers out of the point budget. Lines (roads, paths) are
        not culled by area. 0 is off; the finest tier has no coarser fallback, so it is never culled.
    </p>
    <p class="muted small intro">
        <em>min line (km)</em> is the same declutter for <strong>lines</strong>, and the reason it
        is safe: it measures a road <em>after</em> its OSM fragments have been stitched back into
        one polyline, so it drops the short leftovers stitching could not absorb — junction stubs,
        roundabout arms — rather than a through-road's shortest links. At a coarse tier those are
        far below a pixel yet still cost the renderer a span each; culling them is what buys a
        zoomed-out view enough budget to draw a road network you can orient on. 0 is off, and it
        needs <em>merge lines</em> switched on.
    </p>
    <p class="muted small intro">
        <em>glue fills</em> simplifies a tier's plain area fills as one shared coverage instead of
        one feature at a time, so a boundary two of them share is cut once and neighbours stay
        glued rather than drifting apart into backdrop slivers. It only matters where the tolerance
        is metres wide, and it costs real bake time — the shipped preset turns it on for its two
        coarsest tiers only.
    </p>

    <div class="tiers">
        <div class="hrow small faint">
            <span>tier</span>
            <span>max m/px</span>
            <span>simplify (m)</span>
            <span>min area (px²)</span>
            <span>min line (km)</span>
            <span>glue fills</span>
        </div>
        {#each lods as lod, i (i)}
            <div class="tier">
                <span class="tag">
                    LOD {i}
                    {#if i === 0}<span class="faint small">coarsest</span>
                    {:else if i === lods.length - 1}<span class="faint small">finest</span>{/if}
                </span>
                <span class="cell">
                    {#if i === 0}
                        <span class="inf" title="Coarsest tier — drawn when fully zoomed out">∞</span>
                    {:else}
                        <input
                            type="number"
                            min="0"
                            aria-label="max m/px for LOD {i}"
                            value={lod.max_mpp ?? 0}
                            oninput={(e) => edit(i, "max_mpp", e.currentTarget.value)}
                        />
                    {/if}
                </span>
                <span class="cell">
                    <input
                        type="number"
                        min="0"
                        aria-label="simplify (m) for LOD {i}"
                        value={lod.simplify}
                        oninput={(e) => edit(i, "simplify", e.currentTarget.value)}
                    />
                    {#if lod.simplify === autoSimplify(env.config, i)}
                        <span
                            class="faint small"
                            title="Pixel-accurate default — follows the next tier's max m/px"
                            >auto</span
                        >
                    {:else}
                        <button
                            type="button"
                            class="auto small"
                            title="Reset to the pixel-accurate default (the next tier's max m/px)"
                            onclick={() => edit(i, "simplify", String(autoSimplify(env.config, i)))}
                            >auto: {autoSimplify(env.config, i)}</button
                        >
                    {/if}
                </span>
                <span class="cell">
                    {#if i === lods.length - 1}
                        <span class="faint small" title="The finest tier is never culled — no coarser fallback">—</span>
                    {:else}
                        <input
                            type="number"
                            min="0"
                            aria-label="min area (px²) for LOD {i}"
                            value={lod.min_area_px ?? 0}
                            oninput={(e) => edit(i, "min_area_px", e.currentTarget.value)}
                        />
                    {/if}
                </span>
                <span class="cell">
                    <input
                        type="number"
                        min="0"
                        step="0.1"
                        aria-label="min line (km) for LOD {i}"
                        value={lod.min_line_km ?? 0}
                        oninput={(e) => edit(i, "min_line_km", e.currentTarget.value)}
                    />
                </span>
                <span class="cell">
                    <input
                        type="checkbox"
                        class="glue"
                        aria-label="glue fills for LOD {i}"
                        checked={lod.coverage_simplify === true}
                        onchange={(e) => {
                            setLodCoverageSimplify(env.config, i, e.currentTarget.checked);
                            working.markModified();
                        }}
                    />
                </span>
                {#if lods.length > 1}
                    <button
                        type="button"
                        class="del"
                        title="Remove this tier (feature start tiers are remapped)"
                        onclick={() => {
                            removeLodTier(env.config, i);
                            working.markModified();
                        }}>×</button
                    >
                {/if}
            </div>
        {/each}
    </div>

    <button
        type="button"
        class="btn ghost"
        onclick={() => {
            addLodTier(env.config);
            working.markModified();
        }}
    >
        + add finer level
    </button>
</div>

<style>
    .intro {
        margin: 0 0 10px;
        max-width: 60ch;
    }

    .intro + .intro {
        margin-bottom: 14px;
    }

    .tiers {
        display: flex;
        flex-direction: column;
        gap: 8px;
        margin-bottom: 14px;
    }

    .hrow,
    .tier {
        display: grid;
        grid-template-columns: 110px 120px 200px 130px 120px 90px 1fr;
        gap: 16px;
        align-items: center;
    }

    .hrow {
        padding: 0 13px;
    }

    .tier {
        background: var(--parchment);
        border: 1px solid var(--parchment-3);
        border-radius: 10px;
        padding: 9px 12px;
    }

    .tag {
        font-weight: 600;
        font-size: 13.5px;
        display: inline-flex;
        gap: 6px;
        align-items: baseline;
        white-space: nowrap;
    }

    .cell {
        display: inline-flex;
        align-items: center;
        gap: 8px;
    }

    input {
        width: 82px;
        padding: 4px 7px;
        font-size: 13px;
    }

    input.glue {
        width: auto;
    }

    .inf {
        font-size: 15px;
        color: var(--ink-soft);
        padding: 0 8px;
    }

    .auto {
        background: none;
        border: none;
        padding: 0;
        color: var(--ink-faint);
        text-decoration: underline dotted;
        white-space: nowrap;
    }

    .auto:hover {
        color: var(--forest-deep);
    }

    .del {
        justify-self: end;
        background: none;
        border: none;
        color: var(--ink-faint);
        font-size: 15px;
    }

    .del:hover {
        color: var(--coral);
    }
</style>
