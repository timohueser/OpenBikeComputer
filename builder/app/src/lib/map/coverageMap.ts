// Leaflet glue for the coverage map (#1038). This paints the
// cell catalog's own region boundaries as quiet affordances, each selection
// part's true stair-edged outline, hatched not-baked patches inside the
// selection, and the corridor panel's dashed preview. No grid is ever drawn —
// §8 U1's decision is enforced by this file simply having no code that could.
//
// All Leaflet state lives on the instance (created in onMount, destroyed in
// onDestroy); the Svelte component owns *what* to draw and pushes it through
// the `set*` methods below, so this class stays framework-free and the
// component stays Leaflet-free.
//
// Region picking is resolved by this class rather than by per-polygon click
// handlers, because nested regions overlap: the topmost polygon under the
// cursor is always the smallest, and the whole point of the ancestor ladder is
// that a click means "this ground", not "this smallest boundary". So the
// polygons are never interactive; armed hover and click do point-in-rings over
// every region and hand the component the full chain.

import L from "leaflet";

/** [lat, lon] degrees — Leaflet's own order. */
export type DegPoint = [number, number];

/** One selection part, reduced to what the map draws. */
export interface RenderedPart {
    id: string;
    kind: "region" | "box" | "corridor" | "lasso";
    /** Stair-edged coverage rings, outer + holes, from `coverageRings`. */
    rings: DegPoint[][];
    /** The route itself, for corridor parts. */
    route?: DegPoint[];
    highlighted: boolean;
}

/** Not-baked / partly-baked ground inside the selection, as merged rectangles. */
export interface RenderedWarning {
    /** [[south, west], [north, east]] */
    bounds: [DegPoint, DegPoint];
    kind: "hole" | "partial";
}

/** The corridor panel's uncommitted routes. */
export interface RenderedPreview {
    rings: DegPoint[][];
    routes: DegPoint[][];
}

export interface CoverageMapCallbacks {
    /** An armed click found exactly one region under the point. */
    onRegionPick(regionId: string): void;
    /**
     * An armed click found several nested regions under the point. Smallest
     * first; `defaultId` is the zoom-matched suggestion (the largest chain
     * member wholly inside the current view); `x`/`y` are container pixels for
     * the popup.
     */
    onRegionLadder(regionIds: string[], defaultId: string, x: number, y: number): void;
    /** A box was drawn; bounds in degrees, south/west/north/east. */
    onBoxDrawn(south: number, west: number, north: number, east: number): void;
    /** The in-flight box changed; return the pricing chip's text ("" hides). */
    boxDragLabel(south: number, west: number, north: number, east: number): string;
    /** A lasso ring was drawn, as [lat, lon] degree points, unclosed. */
    onLassoDrawn(points: DegPoint[]): void;
    /** The in-flight lasso changed; return the pricing chip's text ("" hides). */
    lassoDragLabel(points: DegPoint[]): string;
    /** Drawing was cancelled or finished — the component disarms its tool. */
    onDrawEnd(): void;
    /** A hatched warning patch was clicked — zoom is the component's call. */
    onWarningClick(kind: "hole" | "partial"): void;
}

// The selection reads amber (a choice being made), warnings read coral, the
// shelf reads quiet forest — the same assignments as the approved mocks and
// the v1 picker's field-guide palette.
const REGION_STYLE: L.PathOptions = {
    color: "#3c6b39",
    weight: 1.6,
    opacity: 0.75,
    fillColor: "#3c6b39",
    fillOpacity: 0.07,
    interactive: false,
};
const REGION_HOVER_STYLE: L.PathOptions = { weight: 2.2, opacity: 0.9, fillOpacity: 0.1 };
const PART_STYLE: L.PathOptions = {
    color: "#e3ad33",
    weight: 2.5,
    fillColor: "#e3ad33",
    fillOpacity: 0.12,
    interactive: false,
};
const PART_HIGHLIGHT_STYLE: L.PathOptions = { ...PART_STYLE, weight: 4, fillOpacity: 0.2 };
const ROUTE_STYLE: L.PathOptions = { color: "#cf6a2a", weight: 2.5, interactive: false };
const PREVIEW_RING_STYLE: L.PathOptions = {
    color: "#cf6a2a",
    weight: 2,
    dashArray: "7 5",
    fillColor: "#e3ad33",
    fillOpacity: 0.08,
    interactive: false,
};
const PREVIEW_ROUTE_STYLE: L.PathOptions = { ...ROUTE_STYLE, dashArray: "4 4" };
const WARNING_STYLE: Record<"hole" | "partial", L.PathOptions> = {
    hole: { color: "#cf6a2a", weight: 1.6, className: "coverage-hatch" },
    partial: { color: "#cf6a2a", weight: 1, opacity: 0.6, dashArray: "3 3", className: "coverage-hatch faintly" },
};
const BOX_DRAW_STYLE: L.PathOptions = {
    color: "#cf6a2a",
    weight: 2,
    dashArray: "7 5",
    fillColor: "#e3ad33",
    fillOpacity: 0.18,
};
const LASSO_DRAW_STYLE: L.PathOptions = { ...BOX_DRAW_STYLE, dashArray: "5 4" };

/** Even-odd containment in a region's drawable rings, degrees — the same rule
 *  the even-odd fill draws them with, so what hovers is what is painted. */
function pointInDegRings(lat: number, lon: number, rings: DegPoint[][]): boolean {
    let inside = false;
    for (const ring of rings) {
        for (let k = 1; k < ring.length; k++) {
            const [aLat, aLon] = ring[k - 1];
            const [bLat, bLon] = ring[k];
            if (aLat > lat === bLat > lat) continue;
            const crossLon = aLon + ((lat - aLat) / (bLat - aLat)) * (bLon - aLon);
            if (lon < crossLon) inside = !inside;
        }
    }
    return inside;
}

interface ShelfRegion {
    id: string;
    name: string;
    rings: DegPoint[][];
    bounds: L.LatLngBounds;
    /** bbox span, the smallest-first sort key — monotone over nesting. */
    span: number;
}

/** Minimum pointer movement, px, before a lasso takes another vertex. Keeps a
 *  slow gesture from minting thousands of points the pricing then walks. */
const LASSO_STEP_PX = 4;

export class CoverageMapView {
    private map: L.Map;
    private cb: CoverageMapCallbacks;

    private regionLayer: L.LayerGroup = L.layerGroup();
    private regionPolys = new Map<string, L.Polygon>();
    private regions: ShelfRegion[] = [];
    private regionArmed = false;
    private emphasized: string | null = null;
    private hoverTip: L.Tooltip;

    private partLayer: L.LayerGroup = L.layerGroup();
    private warnLayer: L.LayerGroup = L.layerGroup();
    private previewLayer: L.LayerGroup = L.layerGroup();

    private drawArmed = false;
    private drawing = false;
    private drawStart: L.LatLng | null = null;
    private drawRect: L.Rectangle | null = null;
    private drawTip: L.Tooltip;

    private lassoArmed = false;
    private lassoing = false;
    private lassoPoints: L.LatLng[] = [];
    private lassoLastPx: L.Point | null = null;
    private lassoPoly: L.Polygon | null = null;

    private resizeObs: ResizeObserver;
    /** The shelf's bounds. The pane's size settles over several layout passes
     *  (0-wide at mount, a strip, then the real grid track), and a fit computed
     *  against any of the interim sizes frames the wrong world — so the fit is
     *  re-applied on every resize until the user takes over the camera. */
    private shelfBounds: L.LatLngBounds | null = null;
    private userMoved = false;

    constructor(el: HTMLElement, cb: CoverageMapCallbacks) {
        this.cb = cb;
        // boxZoom off: shift+drag stays free for a future gesture and never
        // fights the box tool.
        this.map = L.map(el, { worldCopyJump: true, boxZoom: false }).setView([49, 9], 5);
        L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
            maxZoom: 19,
            attribution: "&copy; OpenStreetMap contributors",
        }).addTo(this.map);

        // Draw order bottom-up: shelf, selection, warnings on top of the
        // selection they annotate, preview above everything settled.
        this.regionLayer.addTo(this.map);
        this.partLayer.addTo(this.map);
        this.warnLayer.addTo(this.map);
        this.previewLayer.addTo(this.map);

        this.drawTip = L.tooltip({ className: "preview-tip", direction: "top", offset: [0, -8] });
        this.hoverTip = L.tooltip({ className: "preview-tip", direction: "top", offset: [0, -12] });

        this.map.on("mousedown", (e) => {
            this.onDrawStart(e);
            this.onLassoStart(e);
        });
        this.map.on("mousemove", (e) => {
            this.onDrawMove(e);
            this.onLassoMove(e);
            this.onRegionHover(e);
        });
        this.map.on("mouseup", () => {
            this.onDrawFinish();
            this.onLassoFinish();
        });
        this.map.on("click", (e) => this.onRegionClick(e));
        this.map.on("mouseout", () => this.clearRegionHover());
        // Leaflet only hears a mouseup inside its container: a drag released
        // over the steps column (or outside the window) used to leave the
        // rubber band armed and drawing (#1041 low sweep). The document hears
        // every release; finishing twice is harmless — `drawing` gates it.
        document.addEventListener("pointerup", this.onDocPointerUp);

        // Any gesture on the pane hands the camera to the user for good.
        el.addEventListener("pointerdown", () => (this.userMoved = true), { capture: true });
        el.addEventListener("wheel", () => (this.userMoved = true), { capture: true, passive: true });

        this.resizeObs = new ResizeObserver(() => {
            this.map.invalidateSize();
            this.tryShelfFit();
        });
        this.resizeObs.observe(el);
        // Defensive (#1041 watch item): one unreproduced sighting of a
        // zero-width SVG viewBox surviving a viewport change — recheck the
        // size whenever the tab becomes visible again, where a stale layout
        // pass would otherwise go unnoticed until a resize.
        document.addEventListener("visibilitychange", this.onVisibility);
    }

    private readonly onDocPointerUp = (): void => {
        if (this.drawing) this.onDrawFinish();
        if (this.lassoing) this.onLassoFinish();
    };

    private readonly onVisibility = (): void => {
        if (document.visibilityState === "visible") {
            this.map.invalidateSize();
            this.tryShelfFit();
        }
    };

    private tryShelfFit(): void {
        if (this.userMoved || !this.shelfBounds) return;
        const size = this.map.getSize();
        if (size.x === 0 || size.y === 0) return;
        this.map.fitBounds(this.shelfBounds);
    }

    destroy(): void {
        this.resizeObs.disconnect();
        document.removeEventListener("pointerup", this.onDocPointerUp);
        document.removeEventListener("visibilitychange", this.onVisibility);
        this.map.remove();
    }

    invalidateSize(): void {
        this.map.invalidateSize();
    }

    // --- the shelf --------------------------------------------------------

    /** The catalog's named regions as quiet outlines. Called once. */
    setRegions(regions: { id: string; name: string; rings: DegPoint[][] }[]): void {
        this.regionLayer.clearLayers();
        this.regionPolys.clear();
        this.regions = [];
        const all = L.latLngBounds([]);
        for (const region of regions) {
            const poly = L.polygon(region.rings, REGION_STYLE);
            poly.addTo(this.regionLayer);
            this.regionPolys.set(region.id, poly);
            const bounds = poly.getBounds();
            this.regions.push({
                ...region,
                bounds,
                span:
                    (bounds.getNorth() - bounds.getSouth()) * (bounds.getEast() - bounds.getWest()),
            });
            all.extend(bounds);
        }
        this.regions.sort((a, b) => a.span - b.span);
        if (all.isValid()) {
            this.shelfBounds = all.pad(0.25);
            this.tryShelfFit();
        }
    }

    /** Arm or disarm region picking: hover and click only resolve regions while
     *  the tool is armed, so plain drag always pans (mock note). */
    setRegionToolArmed(armed: boolean): void {
        if (armed === this.regionArmed) return;
        this.regionArmed = armed;
        if (!armed) this.clearRegionHover();
        this.setCursor(armed || this.drawArmed || this.lassoArmed);
    }

    /** Every region whose boundary contains the point, smallest first. */
    private chainAt(latlng: L.LatLng): ShelfRegion[] {
        return this.regions.filter((r) =>
            r.bounds.contains(latlng) && pointInDegRings(latlng.lat, latlng.lng, r.rings),
        );
    }

    /** Whether a region's whole extent is inside the current view. */
    private fullyVisible(region: ShelfRegion): boolean {
        return this.map.getBounds().contains(region.bounds);
    }

    /**
     * The ladder's zoom-matched suggestion: the largest chain member wholly
     * inside the current view. Looking at all of Europe, that is Germany;
     * zoomed into the Schwarzwald it is Baden-Württemberg. With nothing fully
     * visible the smallest wins — the one certainly near the click.
     */
    private zoomDefault(chain: ShelfRegion[]): ShelfRegion {
        for (let k = chain.length - 1; k >= 0; k--) {
            if (this.fullyVisible(chain[k])) return chain[k];
        }
        return chain[0];
    }

    private onRegionHover(e: L.LeafletMouseEvent): void {
        if (!this.regionArmed || this.drawing || this.lassoing) return;
        const chain = this.chainAt(e.latlng);
        if (chain.length === 0) {
            this.clearRegionHover();
            return;
        }
        const pick = this.zoomDefault(chain);
        this.emphasizeRegion(pick.id);
        const label =
            chain.length > 1 ? `${pick.name} — click to choose the size` : pick.name;
        this.hoverTip.setContent(label).setLatLng(e.latlng);
        if (!this.map.hasLayer(this.hoverTip)) this.hoverTip.addTo(this.map);
    }

    private clearRegionHover(): void {
        this.emphasizeRegion(null);
        if (this.map.hasLayer(this.hoverTip)) this.map.removeLayer(this.hoverTip);
    }

    /** Thicken one region's outline (the hover / ladder-rung preview); null
     *  restores quiet. */
    emphasizeRegion(regionId: string | null): void {
        if (regionId === this.emphasized) return;
        if (this.emphasized) this.regionPolys.get(this.emphasized)?.setStyle(REGION_STYLE);
        this.emphasized = regionId;
        if (regionId) this.regionPolys.get(regionId)?.setStyle(REGION_HOVER_STYLE);
    }

    private onRegionClick(e: L.LeafletMouseEvent): void {
        if (!this.regionArmed || this.drawArmed || this.lassoArmed) return;
        const chain = this.chainAt(e.latlng);
        if (chain.length === 0) return;
        if (chain.length === 1) {
            this.cb.onRegionPick(chain[0].id);
            return;
        }
        const pt = this.map.latLngToContainerPoint(e.latlng);
        this.cb.onRegionLadder(
            chain.map((r) => r.id),
            this.zoomDefault(chain).id,
            pt.x,
            pt.y,
        );
    }

    /** Fly to one region — the popover's or the ladder's pick answers with
     *  where it went. */
    fitRegion(regionId: string): void {
        const poly = this.regionPolys.get(regionId);
        if (!poly) return;
        this.userMoved = true; // deliberate navigation outranks the shelf fit
        this.map.fitBounds(poly.getBounds().pad(0.15));
    }

    /** Whether this region's whole extent is already on screen — the ladder
     *  skips the fly-to for a pick the user can see. */
    regionVisible(regionId: string): boolean {
        const region = this.regions.find((r) => r.id === regionId);
        return region ? this.fullyVisible(region) : false;
    }

    // --- the selection ----------------------------------------------------

    setParts(parts: RenderedPart[]): void {
        this.partLayer.clearLayers();
        for (const part of parts) {
            if (part.rings.length) {
                L.polygon(part.rings, part.highlighted ? PART_HIGHLIGHT_STYLE : PART_STYLE).addTo(this.partLayer);
            }
            if (part.route && part.route.length > 1) {
                L.polyline(part.route, ROUTE_STYLE).addTo(this.partLayer);
            }
        }
    }

    setWarnings(warnings: RenderedWarning[]): void {
        this.warnLayer.clearLayers();
        for (const warning of warnings) {
            const rect = L.rectangle(warning.bounds, WARNING_STYLE[warning.kind]);
            rect.on("click", () => this.cb.onWarningClick(warning.kind));
            rect.bindTooltip(warning.kind === "hole" ? "not baked yet — hole" : "partly baked — detail may stop here", {
                className: "preview-tip",
                sticky: true,
            });
            rect.addTo(this.warnLayer);
        }
        this.ensureHatchPattern();
    }

    setPreview(preview: RenderedPreview | null): void {
        this.previewLayer.clearLayers();
        if (!preview) return;
        // One polygon holding every ring: Leaflet's default even-odd fill rule
        // renders disjoint outers filled and stair-holes empty without this
        // code having to work out which hole belongs to which patch.
        if (preview.rings.length) L.polygon(preview.rings, PREVIEW_RING_STYLE).addTo(this.previewLayer);
        for (const route of preview.routes) {
            if (route.length > 1) L.polyline(route, PREVIEW_ROUTE_STYLE).addTo(this.previewLayer);
        }
    }

    /** Fly to a box — a warning row was clicked. */
    flyTo(south: number, west: number, north: number, east: number): void {
        this.userMoved = true; // deliberate navigation outranks the shelf fit
        this.map.fitBounds(
            [
                [south, west],
                [north, east],
            ],
            { maxZoom: 9, padding: [24, 24] },
        );
    }

    // --- the box tool -----------------------------------------------------

    armBoxDraw(): void {
        this.drawArmed = true;
        this.map.dragging.disable();
        this.setCursor(true);
    }

    cancelBoxDraw(): void {
        this.drawArmed = false;
        this.drawing = false;
        this.removeDrawRect();
        this.map.dragging.enable();
        this.setCursor(this.regionArmed || this.lassoArmed);
    }

    private onDrawStart(e: L.LeafletMouseEvent): void {
        if (!this.drawArmed) return;
        this.drawing = true;
        this.drawStart = e.latlng;
        this.removeDrawRect();
        this.drawRect = L.rectangle(L.latLngBounds(e.latlng, e.latlng), BOX_DRAW_STYLE).addTo(this.map);
    }

    private onDrawMove(e: L.LeafletMouseEvent): void {
        if (!this.drawArmed || !this.drawing || !this.drawRect || !this.drawStart) return;
        const bounds = L.latLngBounds(this.drawStart, e.latlng);
        this.drawRect.setBounds(bounds);
        const label = this.cb.boxDragLabel(
            bounds.getSouth(),
            bounds.getWest(),
            bounds.getNorth(),
            bounds.getEast(),
        );
        this.showDrawTip(label, bounds.getNorthEast());
    }

    private onDrawFinish(): void {
        if (!this.drawArmed || !this.drawing || !this.drawRect) return;
        this.drawing = false;
        const b = this.drawRect.getBounds();
        this.cancelBoxDraw();
        // A stray click or micro-drag draws nothing.
        const nw = this.map.latLngToContainerPoint(b.getNorthWest());
        const se = this.map.latLngToContainerPoint(b.getSouthEast());
        if (Math.abs(nw.x - se.x) < 5 || Math.abs(nw.y - se.y) < 5) {
            this.cb.onDrawEnd();
            return;
        }
        this.cb.onBoxDrawn(b.getSouth(), b.getWest(), b.getNorth(), b.getEast());
        this.cb.onDrawEnd();
    }

    private removeDrawRect(): void {
        if (this.drawRect) {
            this.map.removeLayer(this.drawRect);
            this.drawRect = null;
        }
        if (this.map.hasLayer(this.drawTip)) this.map.removeLayer(this.drawTip);
    }

    private showDrawTip(label: string, at: L.LatLng): void {
        if (label) {
            this.drawTip.setContent(label).setLatLng(at);
            if (!this.map.hasLayer(this.drawTip)) this.drawTip.addTo(this.map);
        } else if (this.map.hasLayer(this.drawTip)) {
            this.map.removeLayer(this.drawTip);
        }
    }

    // --- the lasso tool ---------------------------------------------------

    armLassoDraw(): void {
        this.lassoArmed = true;
        this.map.dragging.disable();
        this.setCursor(true);
    }

    cancelLassoDraw(): void {
        this.lassoArmed = false;
        this.lassoing = false;
        this.removeLassoPoly();
        this.map.dragging.enable();
        this.setCursor(this.regionArmed || this.drawArmed);
    }

    private onLassoStart(e: L.LeafletMouseEvent): void {
        if (!this.lassoArmed) return;
        this.lassoing = true;
        this.lassoPoints = [e.latlng];
        this.lassoLastPx = this.map.latLngToContainerPoint(e.latlng);
        this.removeLassoPoly();
        this.lassoPoly = L.polygon([e.latlng], LASSO_DRAW_STYLE).addTo(this.map);
    }

    private onLassoMove(e: L.LeafletMouseEvent): void {
        if (!this.lassoArmed || !this.lassoing || !this.lassoPoly || !this.lassoLastPx) return;
        const px = this.map.latLngToContainerPoint(e.latlng);
        if (px.distanceTo(this.lassoLastPx) < LASSO_STEP_PX) return;
        this.lassoLastPx = px;
        this.lassoPoints.push(e.latlng);
        this.lassoPoly.setLatLngs(this.lassoPoints);
        const label = this.cb.lassoDragLabel(this.lassoPoints.map((p): DegPoint => [p.lat, p.lng]));
        this.showDrawTip(label, e.latlng);
    }

    private onLassoFinish(): void {
        if (!this.lassoArmed || !this.lassoing) return;
        this.lassoing = false;
        const points = this.lassoPoints;
        this.cancelLassoDraw();
        // A stray click or a gesture with no area draws nothing — the same
        // 5 px threshold the box uses, over the ring's pixel extent.
        let minX = Infinity;
        let minY = Infinity;
        let maxX = -Infinity;
        let maxY = -Infinity;
        for (const p of points) {
            const px = this.map.latLngToContainerPoint(p);
            minX = Math.min(minX, px.x);
            minY = Math.min(minY, px.y);
            maxX = Math.max(maxX, px.x);
            maxY = Math.max(maxY, px.y);
        }
        if (points.length < 3 || maxX - minX < 5 || maxY - minY < 5) {
            this.cb.onDrawEnd();
            return;
        }
        this.cb.onLassoDrawn(points.map((p): DegPoint => [p.lat, p.lng]));
        this.cb.onDrawEnd();
    }

    private removeLassoPoly(): void {
        if (this.lassoPoly) {
            this.map.removeLayer(this.lassoPoly);
            this.lassoPoly = null;
        }
        this.lassoPoints = [];
        this.lassoLastPx = null;
        if (this.map.hasLayer(this.drawTip)) this.map.removeLayer(this.drawTip);
    }

    private setCursor(crosshair: boolean): void {
        this.map.getContainer().classList.toggle("bbox-cursor", crosshair);
    }

    /**
     * The diagonal hatch the warning rectangles fill with. Leaflet's SVG
     * renderer has no pattern support, so the pattern is injected into the
     * overlay pane's `<defs>` once and the rectangles reference it by class
     * (`.coverage-hatch` in app.css) — CSS `fill` beats the presentation
     * attribute Leaflet sets, which is exactly the override needed here.
     */
    private ensureHatchPattern(): void {
        const svg = this.map.getPane("overlayPane")?.querySelector("svg");
        if (!svg || svg.querySelector("#obc-unbaked-hatch")) return;
        const ns = "http://www.w3.org/2000/svg";
        const defs = document.createElementNS(ns, "defs");
        const pattern = document.createElementNS(ns, "pattern");
        pattern.setAttribute("id", "obc-unbaked-hatch");
        pattern.setAttribute("width", "8");
        pattern.setAttribute("height", "8");
        pattern.setAttribute("patternUnits", "userSpaceOnUse");
        const path = document.createElementNS(ns, "path");
        path.setAttribute("d", "M0 8 L8 0");
        path.setAttribute("stroke", "#cf6a2a");
        path.setAttribute("stroke-width", "1.6");
        pattern.appendChild(path);
        defs.appendChild(pattern);
        svg.insertBefore(defs, svg.firstChild);
    }
}
