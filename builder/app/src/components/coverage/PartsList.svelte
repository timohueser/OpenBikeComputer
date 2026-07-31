<script lang="ts">
    // Step 1's ledger of parts (#1038, §8 U2): each composed part — a region,
    // a box, a corridor — as one removable row with its own price.
    //
    // The bytes shown are the part's **gross** bytes: the honest answer to "how
    // big is this part". Cells shared with another part are counted in both, so
    // rows do not sum to the map's total — the row's title says what removing
    // it would actually free (the marginal bytes), which is the other question
    // a ✕ button answers.

    import type { CoverageStore } from "../../lib/coverage/store.svelte";
    import { formatBytes } from "../../lib/format";

    let { store }: { store: CoverageStore } = $props();

    const GLYPH: Record<string, string> = { region: "◧", box: "▭", corridor: "◠" };

    const parts = $derived(store.resolution?.parts ?? []);

    function priceTitle(bytes: number, marginal: number): string {
        if (bytes === marginal) return formatBytes(bytes);
        return `${formatBytes(bytes)} in this part — removing it frees ${formatBytes(marginal)}, the rest is shared with other parts`;
    }
</script>

{#if parts.length === 0}
    <p class="summary muted small">No parts yet — pick a region or draw an area on the map.</p>
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
                <span class="glyph" aria-hidden="true">{GLYPH[p.part.kind]}</span>
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
                    <span class="mono faint small price">pricing…</span>
                {:else}
                    <span class="mono faint small price" title={priceTitle(p.bytes, p.marginalBytes)}>
                        {formatBytes(p.bytes)}
                    </span>
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
