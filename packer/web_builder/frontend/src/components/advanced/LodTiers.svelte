<script lang="ts">
    import { addLodTier, autoSimplify, editLodTier, removeLodTier } from "../../lib/config/edit";
    import { working } from "../../lib/config/storage.svelte";

    const env = $derived(working.envelope!);
    const lods = $derived(env.config.lods);

    function edit(i: number, field: "max_mpp" | "simplify", raw: string) {
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

    <div class="tiers">
        <div class="hrow small faint">
            <span>tier</span>
            <span>max m/px</span>
            <span>simplify (m)</span>
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
        grid-template-columns: 110px 120px 200px 1fr;
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
