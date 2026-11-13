<script lang="ts">
    import type { CoverageStore } from "../../lib/coverage/store.svelte";
    import ToolIcon from "./ToolIcon.svelte";

    let { store }: { store: CoverageStore } = $props();

    const parts = $derived(store.selection.parts);
</script>

{#if parts.length === 0}
    <p class="summary muted small">Choose a region or draw an area on the map.</p>
{:else}
    <ul class="parts">
        {#each parts as part (part.id)}
            {@const resolved = store.resolution?.parts.find((p) => p.part.id === part.id)}
            {@const regionError =
                part.kind === "region" ? (store.regionErrors.get(part.regionId) ?? null) : null}
            <li
                onmouseenter={() => (store.highlightPartId = part.id)}
                onmouseleave={() => {
                    if (store.highlightPartId === part.id) store.highlightPartId = null;
                }}
            >
                <span class="glyph" aria-hidden="true"><ToolIcon kind={part.kind} size={15} /></span>
                <span class="name">{part.name}</span>
                {#if regionError}
                    <span class="small error">{regionError}</span>
                    {#if part.kind === "region" && store.region(part.regionId)}
                        <button type="button" class="retry small" onclick={() => part.kind === "region" && store.retryRegion(part.regionId)}>Retry</button>
                    {/if}
                {:else if !store.indices || resolved?.pending}
                    <span class="mono faint small price">Calculating…</span>
                {/if}
                <button
                    type="button"
                    class="remove"
                    aria-label="Remove {part.name}"
                    title="Remove {part.name}"
                    onclick={() => store.removePart(part.id)}>✕</button
                >
            </li>
        {/each}
    </ul>
{/if}

<style>
    .error { color: var(--coral); flex: 1 1 140px; min-width: 0; overflow-wrap: anywhere; }

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
        flex-wrap: wrap;
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
</style>
