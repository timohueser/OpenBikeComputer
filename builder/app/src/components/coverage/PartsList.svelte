<script lang="ts">
    // Step 1's ledger of parts (#1038, §8 U2): each composed part — a region,
    // a box, a corridor — as one removable row with its own price.
    //
    // The bytes shown are the part's **gross** bytes: the honest answer to "how
    // big is this part". Cells shared with another part are counted in both, so
    // rows do not sum to the map's total — the row's title says what removing
    // it would actually free (the marginal bytes), which is the other question
    // a ✕ button answers.
    //
    // While any corridor part exists, the ledger also carries the **one global
    // corridor width** (#1041 A6, §8 U3's decided shape): the radius is a
    // property of the map, not of the panel that first set it, so the control
    // lives here with the parts it re-buffers — reachable after commit, not
    // locked inside a closed panel. Moving it re-resolves every corridor part
    // live, and each corridor row's price flashes as it re-prices: adjusting a
    // committed map is the feature, and it should look like one, not like a
    // silent mutation. The corridor panel's slider is this same value — one
    // fact, two places, never two widths.

    import type { CoverageStore } from "../../lib/coverage/store.svelte";
    import {
        CORRIDOR_RADIUS_MAX_M,
        CORRIDOR_RADIUS_MIN_M,
    } from "../../lib/coverage/store.svelte";
    import { formatBytes } from "../../lib/format";

    let { store }: { store: CoverageStore } = $props();

    const GLYPH: Record<string, string> = { region: "◧", box: "▭", corridor: "◠" };

    const parts = $derived(store.resolution?.parts ?? []);
    const hasCorridor = $derived(store.selection.parts.some((p) => p.kind === "corridor"));
    const radiusKm = $derived(Math.round(store.selection.corridorRadiusM / 1000));

    function priceTitle(bytes: number, marginal: number): string {
        if (bytes === marginal) return formatBytes(bytes);
        if (marginal === 0) {
            return `${formatBytes(bytes)} in this part — all of it shared with other parts, so removing it frees nothing`;
        }
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
                {:else if p.part.kind === "corridor"}
                    <!-- Keyed by the global width, so a slider move re-mounts
                         the span and its flash animation runs: the re-pricing
                         is visible on the rows it touches (#1041 A6). -->
                    {#key store.selection.corridorRadiusM}
                        <span
                            class="mono faint small price price-flash"
                            title={priceTitle(p.bytes, p.marginalBytes)}
                        >
                            {formatBytes(p.bytes)}
                        </span>
                    {/key}
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

    {#if hasCorridor}
        <div class="width">
            <div class="width-head">
                <label class="small muted" for="corridor-width-global">
                    <span class="glyph" aria-hidden="true">◠</span> Corridor width — all routes
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

    /* A slider move re-mounts each corridor row's price ({#key}), and the
       fresh span runs this once: the global control visibly re-prices the
       rows it touches. */
    .price-flash {
        border-radius: 6px;
        padding: 0 4px;
        margin-right: -4px;
        animation: repriced 0.9s ease-out;
    }

    @keyframes repriced {
        from {
            background: rgba(227, 173, 51, 0.55);
        }
        to {
            background: transparent;
        }
    }

    @media (prefers-reduced-motion: reduce) {
        .price-flash {
            animation: none;
        }
    }
</style>
