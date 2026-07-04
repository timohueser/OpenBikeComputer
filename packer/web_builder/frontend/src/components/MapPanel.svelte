<script lang="ts">
    import { onDestroy, onMount } from "svelte";
    import { api } from "../lib/api/client";
    import { RegionPicker, type Bbox } from "../lib/map/regionPicker";
    import { emptySelection, type AreaSelection } from "../lib/map/selection";

    let {
        active = true,
        onchange,
    }: {
        active?: boolean;
        onchange: (sel: AreaSelection) => void;
    } = $props();

    let mapEl: HTMLDivElement;
    let picker = $state<RegionPicker | null>(null);

    let mode = $state<"regions" | "bbox">("regions");
    let regionIds = $state<string[]>([]);
    let bbox = $state<Bbox | null>(null);
    let drawArmed = $state(false);
    let loadError = $state<string | null>(null);
    let query = $state("");

    // Selection summary derived for the parent on every relevant change.
    function emit() {
        if (!picker) return;
        const sel: AreaSelection = {
            ...emptySelection(),
            mode,
            regionIds,
            regionNames: regionIds.map((id) => picker!.regionName(id)),
            bbox,
        };
        if (mode === "bbox" && bbox) {
            const covering = picker.coveringRegions(bbox);
            sel.coveringIds = covering.map((f) => f.properties.id);
            sel.coveringNames = covering.map((f) => f.properties.name);
            sel.areaKm2 = picker.bboxSummary(bbox);
        }
        onchange(sel);
    }

    const results = $derived(
        query.trim().length >= 2 && picker
            ? picker.regions
                  .filter((f) => f.properties.name.toLowerCase().includes(query.trim().toLowerCase()))
                  .slice(0, 30)
            : [],
    );

    onMount(async () => {
        picker = new RegionPicker(mapEl, {
            onSelectionChange(ids) {
                regionIds = ids;
                emit();
            },
            onBboxChange(b) {
                bbox = b;
                emit();
            },
            onDrawStateChange(armed) {
                drawArmed = armed;
            },
        });
        try {
            picker.setRegions(await api.regions());
        } catch (e) {
            loadError = e instanceof Error ? e.message : String(e);
        }
        emit();
    });

    onDestroy(() => picker?.destroy());

    // The panel lives inside a display-toggled route; Leaflet needs a size
    // recheck when it becomes visible again.
    $effect(() => {
        if (active) picker?.invalidateSize();
    });

    function setMode(m: "regions" | "bbox") {
        if (m === mode) return;
        mode = m;
        picker?.setMode(m);
        if (m === "regions") bbox = null;
        emit();
    }

    function pickSearchResult(id: string) {
        if (!picker) return;
        if (!picker.selected.has(id)) picker.toggleRegion(id);
        picker.fitRegion(id);
        query = "";
    }
</script>

<div class="map-wrap card">
    <div class="map" bind:this={mapEl}></div>

    <div class="overlay top-left">
        {#if mode === "regions"}
            <div class="search">
                <input
                    type="search"
                    placeholder="Search regions (e.g. Germany, Baden)…"
                    bind:value={query}
                />
                {#if results.length}
                    <div class="results">
                        {#each results as f (f.properties.id)}
                            <button type="button" onclick={() => pickSearchResult(f.properties.id)}>
                                <span>{regionIds.includes(f.properties.id) ? "✓ " : ""}{f.properties.name}</span>
                                <span class="mono faint">{f.properties.id}</span>
                            </button>
                        {/each}
                    </div>
                {/if}
            </div>
        {:else}
            <div class="bbox-controls">
                <button type="button" class="btn ghost" onclick={() => (drawArmed ? picker?.cancelDraw() : picker?.armDraw())}>
                    {drawArmed ? "Cancel" : bbox ? "Redraw box" : "Draw box"}
                </button>
                {#if bbox}
                    <button type="button" class="btn ghost" onclick={() => picker?.clearBbox()}>Clear</button>
                {/if}
            </div>
        {/if}
    </div>

    <div class="overlay top-right seg">
        <button type="button" class:active={mode === "regions"} onclick={() => setMode("regions")}>
            Regions
        </button>
        <button type="button" class:active={mode === "bbox"} onclick={() => setMode("bbox")}>
            Draw box
        </button>
    </div>

    {#if mode === "bbox" && !bbox && !drawArmed}
        <div class="overlay bottom-left chip">Click “Draw box”, then drag on the map.</div>
    {/if}

    {#if loadError}
        <div class="overlay bottom-left chip error">Couldn't load regions: {loadError}</div>
    {/if}
</div>

<style>
    .map-wrap {
        position: relative;
        padding: 0;
        overflow: hidden;
        min-height: 480px;
        height: 100%;
    }

    .map {
        position: absolute;
        inset: 0;
    }

    .overlay {
        position: absolute;
        z-index: 1000;
    }

    .top-left {
        top: 12px;
        left: 12px;
    }

    .top-right {
        top: 12px;
        right: 12px;
    }

    .bottom-left {
        bottom: 12px;
        left: 12px;
    }

    .search {
        width: min(290px, 60vw);
    }

    .search input {
        width: 100%;
        border-radius: 999px;
        padding: 7px 14px;
        box-shadow: 0 2px 8px rgba(36, 51, 28, 0.14);
    }

    .results {
        margin-top: 6px;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
        box-shadow: 0 8px 22px rgba(36, 51, 28, 0.18);
        max-height: 280px;
        overflow: auto;
        display: flex;
        flex-direction: column;
    }

    .results button {
        display: flex;
        justify-content: space-between;
        gap: 12px;
        background: none;
        border: none;
        text-align: left;
        padding: 7px 12px;
        font-size: 13px;
        color: var(--ink);
    }

    .results button:hover {
        background: rgba(95, 125, 61, 0.12);
    }

    .bbox-controls {
        display: flex;
        gap: 8px;
    }

    .bbox-controls .btn {
        background: var(--panel);
        box-shadow: 0 2px 8px rgba(36, 51, 28, 0.14);
    }

    .chip.error {
        border-color: var(--coral);
        color: var(--coral);
    }

    /* Leaflet default zoom control sits under our overlays otherwise. */
    :global(.map-wrap .leaflet-top.leaflet-left) {
        top: 58px;
    }
</style>
