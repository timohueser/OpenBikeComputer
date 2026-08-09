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
    //
    // The region tool is armed by default: an empty builder whose map answers
    // no click is a builder that looks broken, and picking a region is the
    // first thing almost every map starts with (2026-08-09 feedback round).

    import { onDestroy, onMount, tick } from "svelte";
    import { coverageRings, mergeCellRects } from "../../lib/catalog/outline";
    import type { RegionEntry } from "../../lib/catalog/manifest";
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
    import type { LatLon } from "../../lib/catalog/selection";
    import CorridorPanel from "./CorridorPanel.svelte";
    import ToolIcon from "./ToolIcon.svelte";

    let { store, active = true }: { store: CoverageStore; active?: boolean } = $props();

    let mapEl: HTMLDivElement;
    let view = $state<CoverageMapView | null>(null);

    type Tool = "none" | "region" | "box" | "corridor" | "lasso";
    let tool = $state<Tool>("region");

    const detailBand = $derived(detailBandId(store.catalog));

    /** [lat, lon] degrees → the catalog's integer microdegrees. */
    const degToLatLon = ([lat, lon]: DegPoint): LatLon => ({
        lat: Math.round(lat * 1e6),
        lon: Math.round(lon * 1e6),
    });

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
            onRegionLadder(regionIds, defaultId, x, y) {
                openLadder(regionIds, defaultId, x, y);
            },
            onBoxDrawn(south, west, north, east) {
                store.addBox(degreesToUbox(south, west, north, east));
            },
            boxDragLabel(south, west, north, east) {
                return priceBox(south, west, north, east);
            },
            onLassoDrawn(points) {
                store.addLasso(points.map(degToLatLon));
            },
            lassoDragLabel(points) {
                return priceLabel(store.priceDraggedLasso(points.map(degToLatLon)));
            },
            onDrawEnd() {
                if (tool === "box" || tool === "lasso") tool = "none";
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

    // The pane lives inside a display-toggled route, so a re-activation
    // rechecks the size. Element resizes are the view's own ResizeObserver's
    // job — a second one here was watching the same element for the same call
    // (#1041 low sweep).
    $effect(() => {
        if (active) view?.invalidateSize();
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

    // --- the ancestor ladder ----------------------------------------------

    /** The click's chain of nested regions, awaiting a pick. `x`/`y` are
     *  container pixels, already clamped into the pane. */
    let ladder = $state<{ ids: string[]; defaultId: string; x: number; y: number } | null>(null);
    let ladderEl = $state<HTMLDivElement>();

    function openLadder(ids: string[], defaultId: string, x: number, y: number) {
        const paneW = mapEl?.clientWidth ?? 0;
        const paneH = mapEl?.clientHeight ?? 0;
        const width = 250;
        const height = 40 + ids.length * 34;
        ladder = {
            ids,
            defaultId,
            x: Math.max(8, Math.min(x, paneW - width - 8)),
            y: Math.max(8, Math.min(y, paneH - height - 8)),
        };
        // The zoom-matched rung takes focus so Enter confirms the suggestion
        // and arrows walk the chain.
        void tick().then(() => {
            ladderEl?.querySelector<HTMLButtonElement>("button.default")?.focus();
        });
    }

    function closeLadder() {
        if (!ladder) return;
        ladder = null;
        view?.emphasizeRegion(null);
    }

    function pickLadder(regionId: string) {
        const visible = view?.regionVisible(regionId) ?? false;
        store.addRegion(regionId);
        // Show what was just added when it reaches beyond the current view —
        // a rung the user can already see needs no camera move.
        if (!visible) view?.fitRegion(regionId);
        closeLadder();
    }

    /** A rung's label: the region, and its price beside it. */
    function ladderRegion(id: string): RegionEntry | undefined {
        return store.region(id);
    }

    // --- the region popover: search + tree --------------------------------

    let query = $state("");
    /** Expanded tree rows, by region id. Replaced wholesale so `$derived`
     *  consumers re-fire. */
    let expanded = $state<ReadonlySet<string>>(new Set());

    const byParent = $derived.by(() => {
        const children = new Map<string | null, RegionEntry[]>();
        for (const region of store.catalog.regions) {
            const list = children.get(region.parent) ?? [];
            list.push(region);
            children.set(region.parent, list);
        }
        for (const list of children.values()) list.sort((a, b) => a.name.localeCompare(b.name));
        return children;
    });

    /** The tree flattened for rendering: each visible row with its depth. */
    const treeRows = $derived.by(() => {
        const rows: { region: RegionEntry; depth: number }[] = [];
        const walk = (parent: string | null, depth: number) => {
            for (const region of byParent.get(parent) ?? []) {
                rows.push({ region, depth });
                if (expanded.has(region.id)) walk(region.id, depth + 1);
            }
        };
        walk(null, 0);
        return rows;
    });

    /** Search results: a flat match over every level, each with its parent
     *  named — "baden" must be findable without knowing where it nests. */
    const searchRows = $derived.by(() => {
        const q = query.trim().toLowerCase();
        if (q.length < 2) return null;
        return store.catalog.regions
            .filter((region) => region.name.toLowerCase().includes(q))
            .slice(0, 30);
    });

    function toggleExpanded(regionId: string) {
        const next = new Set(expanded);
        if (next.has(regionId)) next.delete(regionId);
        else next.add(regionId);
        expanded = next;
    }

    function pickFromList(region: RegionEntry) {
        if (store.hasRegion(region.id)) return;
        store.addRegion(region.id);
        view?.fitRegion(region.id);
    }

    function parentName(region: RegionEntry): string | null {
        return region.parent ? (store.region(region.parent)?.name ?? null) : null;
    }

    // --- tools ------------------------------------------------------------

    let corridorPanel = $state<{ requestClose(): Promise<boolean> }>();
    let corridorBtn = $state<HTMLButtonElement>();

    async function setTool(next: Tool) {
        const target = tool === next ? "none" : next;
        const leavingCorridor = tool === "corridor" && target !== "corridor";
        if (leavingCorridor) {
            // The corridor panel may be holding uploaded routes — the one
            // kind of tool state a user cannot get back by re-arming the tool
            // — so leaving it asks first (#1041 A7). Declining keeps the
            // panel; everything below only runs on a real close.
            if (corridorPanel && !(await corridorPanel.requestClose())) return;
            store.previewParts = [];
        }
        if (tool === "box" && target !== "box") view?.cancelBoxDraw();
        if (tool === "lasso" && target !== "lasso") view?.cancelLassoDraw();
        if (tool === "region" && target !== "region") closeLadder();
        tool = target;
        if (target === "box") view?.armBoxDraw();
        if (target === "lasso") view?.armLassoDraw();
        if (leavingCorridor) {
            // The panel held focus (it takes it on open); its unmount dropped
            // focus on <body>. Hand it back to the tool that opened it — but
            // only when nothing else claimed it, e.g. the rail button a click
            // just landed on.
            await tick();
            if (document.activeElement === document.body) corridorBtn?.focus();
        }
    }

    function onKey(e: KeyboardEvent) {
        if (e.key !== "Escape") return;
        // The ladder is the innermost thing open: Esc peels it first, and only
        // a second Esc disarms the tool.
        if (ladder) {
            closeLadder();
            return;
        }
        if (tool !== "none") void setTool("none");
    }

    // The rail is a toolbar, and a toolbar is ONE tab stop: Tab lands on the
    // remembered tool, arrows walk the tools, Tab leaves (#1041 low sweep,
    // WAI-APG toolbar pattern). Vertical rail, so Up/Down are the axis.
    let railEl = $state<HTMLDivElement>();
    let railAt = $state(0);

    function onRailKey(e: KeyboardEvent) {
        const keys = ["ArrowDown", "ArrowUp", "Home", "End"];
        if (!keys.includes(e.key) || !railEl) return;
        const buttons = [...railEl.querySelectorAll("button")];
        const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
        const next =
            e.key === "ArrowDown"
                ? Math.min(buttons.length - 1, at + 1)
                : e.key === "ArrowUp"
                  ? Math.max(0, at - 1)
                  : e.key === "Home"
                    ? 0
                    : buttons.length - 1;
        buttons[next]?.focus();
        e.preventDefault();
    }

    /** The live pricing chip under a box being drawn: the store prices the
     *  drag through the same resolver + ledger as the released part, so the
     *  chip's number is the row's number by construction (#1041 low sweep). */
    function priceBox(south: number, west: number, north: number, east: number): string {
        return priceLabel(store.priceDraggedBox(degreesToUbox(south, west, north, east)));
    }

    function priceLabel(priced: { bytes: number; cells: number } | { refused: true } | null): string {
        if (!priced) return "";
        if ("refused" in priced) return "too large for one map";
        if (priced.cells === 0) return "nothing baked here yet";
        return `≈ ${formatBytes(priced.bytes)} · ${priced.cells} ${priced.cells === 1 ? "cell" : "cells"}`;
    }

    const partCount = $derived(store.selection.parts.length);
</script>

<svelte:window onkeydown={onKey} />

<div class="map-wrap card">
    <div class="map" bind:this={mapEl}></div>

    <!-- The tool rail (§8 U2): the map owns selection. -->
    <div class="overlay rail-wrap">
        <div
            class="rail"
            role="toolbar"
            aria-label="Selection tools"
            aria-orientation="vertical"
            bind:this={railEl}
        >
            <button
                type="button"
                class:active={tool === "region"}
                aria-pressed={tool === "region"}
                title="Add a region"
                aria-label="Add a region"
                tabindex={railAt === 0 ? 0 : -1}
                onkeydown={onRailKey}
                onfocus={() => (railAt = 0)}
                onclick={() => setTool("region")}><ToolIcon kind="region" /></button
            >
            <button
                type="button"
                class:active={tool === "box"}
                aria-pressed={tool === "box"}
                title="Draw a box"
                aria-label="Draw a box"
                tabindex={railAt === 1 ? 0 : -1}
                onkeydown={onRailKey}
                onfocus={() => (railAt = 1)}
                onclick={() => setTool("box")}><ToolIcon kind="box" /></button
            >
            <button
                type="button"
                class:active={tool === "lasso"}
                aria-pressed={tool === "lasso"}
                title="Lasso an area"
                aria-label="Lasso an area"
                tabindex={railAt === 2 ? 0 : -1}
                onkeydown={onRailKey}
                onfocus={() => (railAt = 2)}
                onclick={() => setTool("lasso")}><ToolIcon kind="lasso" /></button
            >
            <button
                type="button"
                bind:this={corridorBtn}
                class:active={tool === "corridor"}
                aria-pressed={tool === "corridor"}
                title="Add a corridor around a route"
                aria-label="Add a corridor around a route"
                tabindex={railAt === 3 ? 0 : -1}
                onkeydown={onRailKey}
                onfocus={() => (railAt = 3)}
                onclick={() => setTool("corridor")}><ToolIcon kind="corridor" /></button
            >
        </div>
        <span class="rail-hint small faint">region · box · lasso · corridor</span>
    </div>

    {#if tool === "region"}
        <div class="overlay region-list">
            <input
                type="search"
                placeholder="Search regions…"
                aria-label="Search regions"
                bind:value={query}
            />
            {#if searchRows}
                {#if searchRows.length === 0}
                    <p class="small faint head">Nothing named like that is baked yet.</p>
                {/if}
                {#each searchRows as region (region.id)}
                    {@const added = store.hasRegion(region.id)}
                    {@const parent = parentName(region)}
                    <div class="row" style:padding-left="8px">
                        <button
                            type="button"
                            class="name"
                            class:added
                            aria-label={added
                                ? `${region.name} is already in the map`
                                : `Add ${region.name} (${formatBytes(region.bytes)})`}
                            onclick={() => pickFromList(region)}
                        >
                            <span
                                >{added ? "✓ " : ""}{region.name}{#if parent}<span class="faint small">
                                        · {parent}</span
                                    >{/if}</span
                            >
                            <span class="mono faint">{formatBytes(region.bytes)}</span>
                        </button>
                    </div>
                {/each}
            {:else}
                <p class="small faint head">Click a region on the map, or here:</p>
                {#each treeRows as row (row.region.id)}
                    {@const added = store.hasRegion(row.region.id)}
                    {@const kids = byParent.get(row.region.id)?.length ?? 0}
                    <div class="row" style:padding-left={`${row.depth * 16}px`}>
                        {#if kids > 0}
                            <button
                                type="button"
                                class="chev"
                                class:open={expanded.has(row.region.id)}
                                aria-label={expanded.has(row.region.id)
                                    ? `Collapse ${row.region.name}`
                                    : `Expand ${row.region.name} (${kids} inside)`}
                                aria-expanded={expanded.has(row.region.id)}
                                onclick={() => toggleExpanded(row.region.id)}
                            >
                                <svg viewBox="0 0 12 12" aria-hidden="true"
                                    ><path
                                        d="M3 2 L8 6 L3 10"
                                        fill="none"
                                        stroke="currentColor"
                                        stroke-width="1.8"
                                        stroke-linecap="round"
                                    /></svg
                                >
                            </button>
                        {:else}
                            <span class="chev-spacer"></span>
                        {/if}
                        <button
                            type="button"
                            class="name"
                            class:added
                            aria-label={added
                                ? `${row.region.name} is already in the map`
                                : `Add ${row.region.name} (${formatBytes(row.region.bytes)})`}
                            onclick={() => pickFromList(row.region)}
                        >
                            <span>{added ? "✓ " : ""}{row.region.name}</span>
                            <span class="mono faint">{formatBytes(row.region.bytes)}</span>
                        </button>
                    </div>
                {/each}
            {/if}
        </div>
    {/if}

    {#if ladder}
        <div
            class="overlay ladder"
            style:left={`${ladder.x}px`}
            style:top={`${ladder.y}px`}
            bind:this={ladderEl}
            role="menu"
            aria-label="Add to the map"
        >
            <p class="small faint head">Add to the map — smallest first</p>
            {#each ladder.ids as id (id)}
                {@const region = ladderRegion(id)}
                {#if region}
                    <button
                        type="button"
                        role="menuitem"
                        class:default={id === ladder.defaultId}
                        class:added={store.hasRegion(id)}
                        onmouseenter={() => view?.emphasizeRegion(id)}
                        onmouseleave={() => view?.emphasizeRegion(null)}
                        onfocus={() => view?.emphasizeRegion(id)}
                        onclick={() => pickLadder(id)}
                    >
                        <span>{store.hasRegion(id) ? "✓ " : ""}{region.name}</span>
                        <span class="mono faint">{formatBytes(region.bytes)}</span>
                    </button>
                {/if}
            {/each}
        </div>
    {/if}

    {#if tool === "corridor"}
        <div class="overlay corridor">
            <CorridorPanel bind:this={corridorPanel} {store} onclose={() => void setTool("none")} />
        </div>
    {/if}

    {#if store.drawError}
        <div class="overlay bottom-left chip error">{store.drawError}</div>
    {:else if tool === "box"}
        <div class="overlay bottom-left chip">Drag to draw a box — Esc cancels.</div>
    {:else if tool === "lasso"}
        <div class="overlay bottom-left chip">Drag to draw around an area — Esc cancels.</div>
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
        display: inline-flex;
        align-items: center;
        justify-content: center;
        transition:
            background 0.15s,
            color 0.15s;
    }

    .rail button:hover:not(.active) {
        background: var(--parchment-2);
        color: var(--ink);
    }

    .rail button.active {
        background: var(--forest);
        color: var(--panel);
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
        width: min(300px, 60vw);
        max-height: min(440px, calc(100% - 24px));
        overflow: auto;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
        box-shadow: 0 8px 22px rgba(36, 51, 28, 0.18);
        display: flex;
        flex-direction: column;
        padding: 6px;
    }

    .region-list input {
        margin: 2px 2px 6px;
        padding: 6px 10px;
        font-size: 13px;
    }

    .region-list .head {
        margin: 4px 8px 6px;
    }

    .region-list .row {
        display: flex;
        align-items: center;
        gap: 2px;
    }

    .region-list .chev {
        flex: none;
        width: 20px;
        height: 20px;
        border: none;
        background: none;
        color: var(--ink-faint);
        padding: 0;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        border-radius: 5px;
    }

    .region-list .chev svg {
        width: 11px;
        height: 11px;
        transition: transform 0.12s;
    }

    .region-list .chev.open svg {
        transform: rotate(90deg);
    }

    .region-list .chev:hover {
        background: rgba(95, 125, 61, 0.12);
        color: var(--ink);
    }

    .chev-spacer {
        flex: none;
        width: 20px;
    }

    .region-list button.name {
        flex: 1;
        display: flex;
        justify-content: space-between;
        gap: 12px;
        background: none;
        border: none;
        text-align: left;
        padding: 6px 8px;
        border-radius: 7px;
        font-size: 13px;
        color: var(--ink);
        min-width: 0;
    }

    .region-list button.name > span:first-child {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .region-list button.name:hover {
        background: rgba(95, 125, 61, 0.12);
    }

    .region-list button.name.added {
        color: var(--forest-deep);
        font-weight: 600;
        cursor: default;
    }

    .ladder {
        width: 250px;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
        box-shadow: 0 8px 22px rgba(36, 51, 28, 0.22);
        padding: 6px;
        display: flex;
        flex-direction: column;
    }

    .ladder .head {
        margin: 3px 8px 4px;
    }

    .ladder button {
        display: flex;
        justify-content: space-between;
        align-items: baseline;
        gap: 10px;
        background: none;
        border: none;
        text-align: left;
        padding: 7px 9px;
        border-radius: 7px;
        font-size: 13.5px;
        color: var(--ink);
    }

    .ladder button:hover,
    .ladder button:focus-visible {
        background: rgba(95, 125, 61, 0.14);
        outline: none;
    }

    .ladder button.default {
        font-weight: 600;
    }

    .ladder button.added {
        color: var(--forest-deep);
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

    /* At phone widths the attribution line spans nearly the whole pane, so
       the chip moves up a line instead of sitting on top of it (#1041 low
       sweep, mobile). */
    @media (max-width: 940px) {
        .bottom-left {
            bottom: 38px;
        }
    }

    .chip.error {
        border-color: var(--coral);
        color: var(--coral);
    }

    /* app.css drops Leaflet's zoom control 58px for the old panel's search bar;
       this pane has the tool rail there instead. The offset tucks the control
       directly under the rail (4 × 32px buttons + gaps + padding + 12px top
       ≈ 160px) rather than parking it mid-pane — which is where a fixed
       200px put it on a short phone pane (#1041 low sweep, mobile). */
    .map-wrap :global(.leaflet-top.leaflet-left) {
        top: 172px;
    }
</style>
