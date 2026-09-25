<script lang="ts">
    import type { CoverageStore } from "../../lib/coverage/store.svelte";
    import {
        CORRIDOR_RADIUS_MAX_M,
        CORRIDOR_RADIUS_MIN_M,
    } from "../../lib/coverage/store.svelte";
    import ToolIcon from "./ToolIcon.svelte";

    let { store }: { store: CoverageStore } = $props();

    const parts = $derived(store.resolution?.parts ?? []);
    const hasCorridor = $derived(store.selection.parts.some((p) => p.kind === "corridor"));
    const radiusKm = $derived(Math.round(store.selection.corridorRadiusM / 1000));
</script>

{#if parts.length === 0}
    <p class="summary muted small">Choose a region or draw an area on the map.</p>
{:else}
    <ul class="parts">
        {#each parts as p (p.part.id)}
            {@const regionError =
                p.part.kind === "region" ? (store.regionErrors.get(p.part.regionId) ?? null) : null}
            <li
                onmouseenter={() => (store.highlightPartId = p.part.id)}
                onmouseleave={() => {
                    if (store.highlightPartId === p.part.id) store.highlightPartId = null;
                }}
            >
                <span class="glyph" aria-hidden="true"><ToolIcon kind={p.part.kind} size={15} /></span>
                <span class="name">{p.part.kind === "corridor" ? `Corridor — ${p.part.name}` : p.part.name}</span>
                {#if regionError}
                    <button
                        type="button"
                        class="retry small"
                        title={regionError}
                        onclick={() => p.part.kind === "region" && store.retryRegion(p.part.regionId)}
                    >
                        failed — retry
                    </button>
                {:else if p.pending}
                    <span class="mono faint small price">Calculating…</span>
                {/if}
                <button
                    type="button"
                    class="remove"
                    aria-label="Remove {p.part.name}"
                    title="Remove {p.part.name}"
                    onclick={() => store.removePart(p.part.id)}>✕</button
                >
            </li>
        {/each}
    </ul>

    {#if hasCorridor}
        <div class="width">
            <div class="width-head">
                <label class="small muted" for="corridor-width-global">
                    <span class="glyph" aria-hidden="true"><ToolIcon kind="corridor" size={13} /></span> Corridor width — all routes
                </label>
                <span class="mono small">± {radiusKm} km</span>
            </div>
            <input
                id="corridor-width-global"
                type="range"
                min={CORRIDOR_RADIUS_MIN_M / 1000}
                max={CORRIDOR_RADIUS_MAX_M / 1000}
                step="1"
                value={radiusKm}
                oninput={(e) =>
                    store.setCorridorRadius(Number((e.currentTarget as HTMLInputElement).value) * 1000)}
            />
        </div>
    {/if}
{/if}

<style>
    .summary {
        margin: 0;
        font-size: 14px;
    }

    .parts {
        list-style: none;
        margin: 0;
        padding: 0;
        display: flex;
        flex-direction: column;
        gap: 5px;
    }

    .parts li {
        display: flex;
        align-items: center;
        gap: 8px;
        background: var(--parchment);
        border: 1px solid var(--line);
        border-radius: 8px;
        padding: 6px 10px;
        transition: border-color 0.15s;
    }

    .parts li:hover {
        border-color: var(--wood);
    }

    .glyph {
        color: var(--ink-soft);
        flex: none;
        display: inline-flex;
        align-items: center;
    }

    .name {
        flex: 1;
        font-size: 13.5px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .price {
        flex: none;
    }

    .retry {
        background: none;
        border: none;
        color: var(--coral);
        padding: 0;
        text-decoration: underline;
    }

    .remove {
        background: none;
        border: none;
        color: var(--ink-faint);
        padding: 0 2px;
        font-size: 14px;
        flex: none;
    }

    .remove:hover {
        color: var(--coral);
    }

    /* The one-map-wide corridor width, as a quiet appendix to the corridor
       rows it re-buffers: same parchment as a row, dashed border so it reads
       as a control over the parts rather than another part. */
    .width {
        background: var(--parchment);
        border: 1px dashed var(--line);
        border-radius: 8px;
        padding: 6px 10px 8px;
        margin-top: 1px;
    }

    .width-head {
        display: flex;
        justify-content: space-between;
        align-items: baseline;
        gap: 8px;
        margin-bottom: 2px;
    }

    .width input[type="range"] {
        width: 100%;
        accent-color: var(--forest);
        padding: 0;
        border: none;
        background: none;
    }
</style>
