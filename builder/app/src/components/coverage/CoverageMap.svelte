<script lang="ts">
    // The coverage step's map pane (#1038): tool rail, quiet region
    // affordances, each part's true stair outline, hatched not-baked ground,
    // and the corridor panel — the approved R2·1/R2·2 frames as a component.
    //
    // Division of labour: `CoverageMapView` owns Leaflet, `CoverageStore` owns
    // the selection, and this component is the adapter that turns one into
    // drawing instructions for the other. The one piece of real work it keeps
    // is the ring cache — stair outlines are geometry over every cell of a
    // part, worth computing once per distinct cell set rather than once per
    // resolution object identity.

    import { onDestroy, onMount } from "svelte";
    import { cellsIntersecting, formatCellId } from "../../lib/catalog/v2/grid";
    import { coverageRings, mergeCellRects } from "../../lib/catalog/v2/outline";
    import {
        degreesToUbox,
        detailBandId,
        mergeMixedCellRects,
        parseCells,
        ringToDegrees,
        uboxToDegrees,
    } from "../../lib/coverage/shape";
    import type { CoverageStore } from "../../lib/coverage/store.svelte";
    import { formatBytes } from "../../lib/format";
    import {
        CoverageMapView,
        type DegPoint,
        type RenderedPart,
        type RenderedWarning,
    } from "../../lib/map/coverageMap";
    import CorridorPanel from "./CorridorPanel.svelte";

    let { store, active = true }: { store: CoverageStore; active?: boolean } = $props();

    let mapEl: HTMLDivElement;
    let view = $state<CoverageMapView | null>(null);

    type Tool = "none" | "region" | "box" | "corridor";
    let tool = $state<Tool>("none");

    const detailBand = $derived(detailBandId(store.catalog));

    // --- outline cache ----------------------------------------------------
    // Keyed by part id, validated by comparing the cell list itself: the
    // resolver hands fresh array copies every resolve, so identity is useless
    // but content comparison is O(n) over short strings — far cheaper than
    // re-walking the boundary of a country-sized part.
    const ringCache = new Map<string, { cells: string[]; rings: DegPoint[][] }>();

    function sameCells(a: string[], b: string[]): boolean {
        if (a.length !== b.length) return false;
        for (let k = 0; k < a.length; k++) if (a[k] !== b[k]) return false;
        return true;
    }

    function ringsFor(key: string, cells: string[]): DegPoint[][] {
        const hit = ringCache.get(key);
        if (hit && sameCells(hit.cells, cells)) return hit.rings;
        const rings = coverageRings(parseCells(cells)).map(ringToDegrees);
        ringCache.set(key, { cells, rings });
        return rings;
    }

    onMount(() => {
        view = new CoverageMapView(mapEl, {
            onRegionPick(regionId) {
                store.addRegion(regionId);
            },
            onBoxDrawn(south, west, north, east) {
                store.addBox(degreesToUbox(south, west, north, east));
            },
            boxDragLabel(south, west, north, east) {
                return priceBox(south, west, north, east);
            },
            onDrawEnd() {
                if (tool === "box") tool = "none";
            },
            onWarningClick(kind) {
                store.focusWarnings(kind);
            },
        });
        view.setRegions(
            store.catalog.regions.map((r) => ({
                id: r.id,
                name: r.name,
                rings: r.boundary.rings.map(ringToDegrees),
            })),
        );
    });

    onDestroy(() => view?.destroy());

    // The pane lives inside a display-toggled route and a viewport-locked
    // layout — same two reasons the v1 panel rechecks its size.
    $effect(() => {
        if (active) view?.invalidateSize();
    });
    $effect(() => {
        const observer = new ResizeObserver(() => view?.invalidateSize());
        observer.observe(mapEl);
        return () => observer.disconnect();
    });

    // --- selection → drawing ---------------------------------------------

    $effect(() => {
        const resolution = store.resolution;
        if (!view) return;
        const parts: RenderedPart[] = [];
        const live = new Set<string>();
        for (const partRes of resolution?.parts ?? []) {
            const id = partRes.part.id;
            live.add(id);
            const cells = partRes.cellsByBand.get(detailBand) ?? [];
            parts.push({
                id,
                kind: partRes.part.kind,
                rings: ringsFor(id, cells),
                route:
                    partRes.part.kind === "corridor"
                        ? partRes.part.points.map((p): DegPoint => [p.lat / 1e6, p.lon / 1e6])
                        : undefined,
                highlighted: store.highlightPartId === id,
            });
        }
        for (const key of [...ringCache.keys()]) {
            if (!live.has(key) && !key.startsWith("preview ")) ringCache.delete(key);
        }
        view.setParts(parts);
    });

    // Warnings, hatched inside the selection. Holes come from **every** band
    // (#1041 A5): a missing coarse cell is a map with no zoomed-out context
    // there, as real as a missing street grid, so its square hatches too.
    // Partial cells stay the detail band's — and only where they abut a hole
    // (#1041 A9): `store.partialHatchCells` owns that rule and says why. The
    // hole hatch stays the louder of the two (solid ring, denser fill); the
    // partial hatch reads as its quieter margin.
    $effect(() => {
        if (!view) return;
        const warnings: RenderedWarning[] = [];
        for (const rect of mergeMixedCellRects(store.holeCells())) {
            warnings.push({ bounds: uboxToDegrees(rect), kind: "hole" });
        }
        for (const rect of mergeCellRects(parseCells(store.partialHatchCells()))) {
            warnings.push({ bounds: uboxToDegrees(rect), kind: "partial" });
        }
        view.setWarnings(warnings);
    });

    // The corridor panel's dashed preview.
    $effect(() => {
        const previewed = store.previewResolution;
        if (!view) return;
        if (!previewed || store.previewParts.length === 0) {
            view.setPreview(null);
            return;
        }
        const cells = new Set<string>();
        for (const partRes of previewed.parts) {
            if (!partRes.part.id.startsWith("preview-")) continue;
            for (const id of partRes.cellsByBand.get(detailBand) ?? []) cells.add(id);
        }
        view.setPreview({
            rings: ringsFor("preview *", [...cells].sort()),
            routes: store.previewParts.map((p) => p.points.map((q): DegPoint => [q.lat / 1e6, q.lon / 1e6])),
        });
    });

    // A warning row (here or in the summary card) asked for a look.
    $effect(() => {
        const box = store.focus;
        if (!box || !view) return;
        const [[s, w], [n, e]] = uboxToDegrees(box);
        view.flyTo(s, w, n, e);
        store.focus = null;
    });

    $effect(() => {
        view?.setRegionToolArmed(tool === "region");
    });

    // --- tools ------------------------------------------------------------

    let corridorPanel = $state<{ requestClose(): Promise<boolean> }>();

    async function setTool(next: Tool) {
        const target = tool === next ? "none" : next;
        if (tool === "corridor" && target !== "corridor") {
            // The corridor panel may be holding uploaded routes — the one
            // kind of tool state a user cannot get back by re-arming the tool
            // — so leaving it asks first (#1041 A7). Declining keeps the
            // panel; everything below only runs on a real close.
            if (corridorPanel && !(await corridorPanel.requestClose())) return;
            store.previewParts = [];
        }
        if (tool === "box" && target !== "box") view?.cancelBoxDraw();
        tool = target;
        if (target === "box") view?.armBoxDraw();
    }

    function onKey(e: KeyboardEvent) {
        if (e.key === "Escape" && tool !== "none") {
            void setTool("none");
        }
    }

    /** The live pricing chip under a box being drawn: real published bytes
     *  across every band, the same numbers the part will cost once released. */
    function priceBox(south: number, west: number, north: number, east: number): string {
        const indices = store.indices;
        if (!indices) return "";
        const box = degreesToUbox(south, west, north, east);
        let bytes = 0;
        let count = 0;
        try {
            for (const b of store.catalog.schema.bands) {
                const index = indices.get(b.id);
                if (!index) continue;
                for (const cell of cellsIntersecting(b.cell_log2, box)) {
                    const entry = index.byId.get(formatCellId(cell));
                    if (entry) {
                        bytes += entry.bytes;
                        count += 1;
                    }
                }
            }
        } catch {
            return "too large for one map";
        }
        return count ? `≈ ${formatBytes(bytes)} · ${count} cells` : "nothing baked here yet";
    }

    const partCount = $derived(store.selection.parts.length);
</script>

<svelte:window onkeydown={onKey} />

<div class="map-wrap card">
    <div class="map" bind:this={mapEl}></div>

    <!-- The tool rail (§8 U2): the map owns selection. Lasso is a ghosted slot,
         not a hidden one — the rail is the one place tools live, and showing
         where the next one goes costs a single quiet button. -->
    <div class="overlay rail-wrap">
        <div class="rail" role="toolbar" aria-label="Selection tools">
            <button
                type="button"
                class:active={tool === "region"}
                aria-pressed={tool === "region"}
                title="Add a region"
                onclick={() => setTool("region")}>◧</button
            >
            <button
                type="button"
                class:active={tool === "box"}
                aria-pressed={tool === "box"}
                title="Draw a box"
                onclick={() => setTool("box")}>▭</button
            >
            <button
                type="button"
                class:active={tool === "corridor"}
                aria-pressed={tool === "corridor"}
                title="Add a corridor around a route"
                onclick={() => setTool("corridor")}>◠</button
            >
            <button type="button" class="ghosted" disabled title="Lasso — later">◌</button>
        </div>
        <span class="rail-hint small faint">regions · box · corridor</span>
    </div>

    {#if tool === "region"}
        <div class="overlay region-list">
            <p class="small faint head">Click a region on the map, or here:</p>
            {#each store.catalog.regions as region (region.id)}
                {@const added = store.hasRegion(region.id)}
                <button
                    type="button"
                    class:added
                    aria-label={added
                        ? `${region.name} is already in the map`
                        : `Add ${region.name} (${formatBytes(region.bytes)})`}
                    onclick={() => {
                        if (added) return;
                        store.addRegion(region.id);
                        view?.fitRegion(region.id);
                    }}
                >
                    <span>{added ? "✓ " : ""}{region.name}</span>
                    <span class="mono faint">{formatBytes(region.bytes)}</span>
                </button>
            {/each}
        </div>
    {/if}

    {#if tool === "corridor"}
        <div class="overlay corridor">
            <CorridorPanel bind:this={corridorPanel} {store} onclose={() => void setTool("none")} />
        </div>
    {/if}

    {#if store.boxError}
        <div class="overlay bottom-left chip error">{store.boxError}</div>
    {:else if tool === "box"}
        <div class="overlay bottom-left chip">Drag to draw a box — Esc cancels.</div>
    {:else if tool === "region"}
        <div class="overlay bottom-left chip">Every region you click joins the map — Esc when done.</div>
    {:else if partCount === 0 && tool === "none"}
        <div class="overlay bottom-left chip">
            Pick a region or draw an area — each part you add joins one map.
        </div>
    {/if}
</div>

<style>
    .map-wrap {
        position: relative;
        padding: 0;
        overflow: hidden;
        height: 100%;
        min-height: 0;
    }

    @media (max-width: 940px) {
        .map-wrap {
            min-height: 480px;
        }
    }

    .map {
        position: absolute;
        inset: 0;
    }

    .overlay {
        position: absolute;
        z-index: 1000;
    }

    .rail-wrap {
        top: 12px;
        left: 12px;
        display: flex;
        align-items: flex-start;
        gap: 8px;
    }

    .rail {
        display: flex;
        flex-direction: column;
        gap: 4px;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 10px;
        padding: 4px;
        box-shadow: 0 2px 8px rgba(36, 51, 28, 0.14);
    }

    .rail button {
        width: 32px;
        height: 32px;
        border: none;
        border-radius: 7px;
        background: var(--parchment);
        color: var(--ink-soft);
        font-size: 15px;
        line-height: 1;
        transition:
            background 0.15s,
            color 0.15s;
    }

    .rail button:hover:not(:disabled):not(.active) {
        background: var(--parchment-2);
        color: var(--ink);
    }

    .rail button.active {
        background: var(--forest);
        color: var(--panel);
    }

    .rail button.ghosted {
        opacity: 0.45;
        cursor: default;
    }

    .rail button:focus-visible {
        outline: 2px solid var(--amber);
        outline-offset: 1px;
    }

    .rail-hint {
        margin-top: 4px;
        background: var(--panel);
        border-radius: 8px;
        padding: 2px 8px;
        box-shadow: 0 2px 8px rgba(36, 51, 28, 0.1);
    }

    .region-list {
        top: 12px;
        left: 64px;
        width: min(280px, 60vw);
        max-height: min(420px, calc(100% - 24px));
        overflow: auto;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
        box-shadow: 0 8px 22px rgba(36, 51, 28, 0.18);
        display: flex;
        flex-direction: column;
        padding: 6px;
    }

    .region-list .head {
        margin: 4px 8px 6px;
    }

    .region-list button {
        display: flex;
        justify-content: space-between;
        gap: 12px;
        background: none;
        border: none;
        text-align: left;
        padding: 7px 10px;
        border-radius: 7px;
        font-size: 13px;
        color: var(--ink);
    }

    .region-list button:hover {
        background: rgba(95, 125, 61, 0.12);
    }

    .region-list button.added {
        color: var(--forest-deep);
        font-weight: 600;
        cursor: default;
    }

    .corridor {
        top: 12px;
        left: 64px;
        max-height: calc(100% - 24px);
        display: flex;
    }

    .bottom-left {
        bottom: 12px;
        left: 12px;
        max-width: min(420px, calc(100% - 24px));
    }

    .chip.error {
        border-color: var(--coral);
        color: var(--coral);
    }

    /* app.css drops Leaflet's zoom control 58px for the v1 panel's search bar;
       this pane has the tool rail there instead, which is taller. */
    .map-wrap :global(.leaflet-top.leaflet-left) {
        top: 200px;
    }
</style>
