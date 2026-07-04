<script lang="ts">
    import { addLodTier, removeLodTier } from "../../lib/config/edit";
    import { working } from "../../lib/config/storage.svelte";

    const env = $derived(working.envelope!);
    const lods = $derived(env.config.lods);

    function edit(i: number, field: "max_mpp" | "simplify", raw: string) {
        const v = parseFloat(raw);
        lods[i][field] = Number.isFinite(v) ? v : 0;
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

    <div class="tiers">
        {#each lods as lod, i (i)}
            <div class="tier">
                <span class="tag">
                    LOD {i}
                    {#if i === 0}<span class="faint small">coarsest</span>
                    {:else if i === lods.length - 1}<span class="faint small">finest</span>{/if}
                </span>
                <label class="small muted">
                    max m/px
                    {#if i === 0}
                        <span class="inf" title="Coarsest tier — drawn when fully zoomed out">∞</span>
                    {:else}
                        <input
                            type="number"
                            min="0"
                            value={lod.max_mpp ?? 0}
                            oninput={(e) => edit(i, "max_mpp", e.currentTarget.value)}
                        />
                    {/if}
                </label>
                <label class="small muted">
                    simplify (m)
                    <input
                        type="number"
                        min="0"
                        value={lod.simplify}
                        oninput={(e) => edit(i, "simplify", e.currentTarget.value)}
                    />
                </label>
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
        margin: 0 0 14px;
        max-width: 60ch;
    }

    .tiers {
        display: flex;
        flex-direction: column;
        gap: 8px;
        margin-bottom: 14px;
    }

    .tier {
        display: flex;
        align-items: center;
        gap: 16px;
        background: var(--parchment);
        border: 1px solid var(--parchment-3);
        border-radius: 10px;
        padding: 9px 12px;
    }

    .tag {
        font-weight: 600;
        font-size: 13.5px;
        min-width: 96px;
        display: inline-flex;
        gap: 6px;
        align-items: baseline;
    }

    label {
        display: inline-flex;
        align-items: center;
        gap: 7px;
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

    .del {
        margin-left: auto;
        background: none;
        border: none;
        color: var(--ink-faint);
        font-size: 15px;
    }

    .del:hover {
        color: var(--coral);
    }
</style>
