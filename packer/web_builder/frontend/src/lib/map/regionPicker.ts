// Leaflet glue for the area picker: region click-to-pick with hover preview,
// plus the mutually-exclusive bounding-box mode (one-shot draw, corner-handle
// resize, body drag). Logic ported from the legacy app.js; all Leaflet state
// lives on the instance (created in onMount, destroyed in onDestroy) — no
// module-level singletons, so navigation can't double-initialize the map.

import L from "leaflet";
import {
    bboxAreaKm2,
    featureContains,
    indexRegions,
    regionsForBbox,
    smallestRegionAt,
    type IndexedRegion,
} from "./geo";
import type { RegionFeature } from "../api/client";

export type Bbox = [number, number, number, number]; // [W, S, E, N] degrees

export interface PickerCallbacks {
    onSelectionChange(ids: string[]): void;
    onBboxChange(bbox: Bbox | null): void;
    onDrawStateChange(armed: boolean): void;
}

// Field-guide colors: selection reads forest (kept), preview/box read coral
// (transient), matching the docs palette.
const SELECT_STYLE = { color: "#3c6b39", weight: 2, fillColor: "#3c6b39", fillOpacity: 0.22 };
const PREVIEW_STYLE = {
    color: "#cf6a2a",
    weight: 2,
    dashArray: "5,4",
    fillColor: "#cf6a2a",
    fillOpacity: 0.12,
    interactive: false,
};
const BBOX_STYLE = { color: "#cf6a2a", weight: 2, fillColor: "#cf6a2a", fillOpacity: 0.08 };

const CORNERS = ["nw", "ne", "se", "sw"] as const;
type Corner = (typeof CORNERS)[number];
const OPPOSITE: Record<Corner, Corner> = { nw: "se", ne: "sw", se: "nw", sw: "ne" };

function cornerLatLng(b: L.LatLngBounds, key: Corner): L.LatLng {
    return key === "nw"
        ? b.getNorthWest()
        : key === "ne"
          ? b.getNorthEast()
          : key === "se"
            ? b.getSouthEast()
            : b.getSouthWest();
}

export class RegionPicker {
    regions: IndexedRegion[] = [];
    readonly selected = new Set<string>();

    private map: L.Map;
    private cb: PickerCallbacks;
    private regionsById = new Map<string, IndexedRegion>();
    private highlightLayer: L.GeoJSON | null = null;

    private bboxMode = false;
    private bboxRect: L.Rectangle | null = null;
    private bboxBounds: L.LatLngBounds | null = null;
    private bboxHandles: Partial<Record<Corner, L.Marker>> | null = null;
    private bboxDrawing = false;
    private bboxStart: L.LatLng | null = null;
    private drawArmed = false;

    private previewLayer: L.GeoJSON | null = null;
    private previewId: string | null = null;
    private previewTip: L.Tooltip;
    private pendingLatLng: L.LatLng | null = null;
    private rafPending = false;
    private popupOpen = false;
    private resizeObs: ResizeObserver;

    constructor(el: HTMLElement, cb: PickerCallbacks) {
        this.cb = cb;
        // boxZoom off: shift+drag is our draw-a-bbox gesture instead.
        this.map = L.map(el, { worldCopyJump: true, boxZoom: false }).setView([30, 10], 2);
        L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
            maxZoom: 19,
            attribution: "&copy; OpenStreetMap contributors",
        }).addTo(this.map);
        this.previewTip = L.tooltip({ className: "preview-tip", direction: "top", offset: [0, -6] });

        this.map.on("click", (e) => this.onClick(e));
        this.map.on("mousemove", (e) => this.onMouseMove(e));
        this.map.on("mouseout", () => {
            if (!this.popupOpen) {
                this.pendingLatLng = null;
                this.clearPreview();
            }
        });
        this.map.on("popupclose", () => {
            this.popupOpen = false;
            this.clearPreview();
        });
        this.map.on("mousedown", (e) => this.onDrawStart(e));
        this.map.on("mouseup", () => this.onDrawEnd());

        // Keyboard: Esc cancels an armed/mid-draw box; arrows nudge a finished
        // box (Shift+arrows drag its south-east corner). Bound to the map
        // container so it only fires when the map has focus.
        el.tabIndex = 0;
        el.addEventListener("keydown", (e) => this.onKey(e));

        // Leaflet only re-measures on window resize; layout-driven size changes
        // (grid breakpoints, panels, a container that was 0-wide at mount)
        // need an explicit nudge or every projection is subtly wrong.
        this.resizeObs = new ResizeObserver(() => this.map.invalidateSize());
        this.resizeObs.observe(el);

        // Shift+drag draws a box without pressing "Draw box" first. Captured
        // before Leaflet's own mousedown listeners so the map never starts a
        // pan for this gesture.
        el.addEventListener(
            "mousedown",
            (ev) => {
                if (!this.bboxMode || this.drawArmed || !ev.shiftKey || ev.button !== 0) return;
                ev.stopPropagation();
                ev.preventDefault();
                this.armDraw();
                const latlng = this.map.mouseEventToLatLng(ev);
                this.bboxDrawing = true;
                this.bboxStart = latlng;
                this.removeBox();
                this.bboxRect = L.rectangle(L.latLngBounds(latlng, latlng), BBOX_STYLE).addTo(this.map);
            },
            true,
        );
    }

    destroy() {
        this.resizeObs.disconnect();
        this.map.remove();
    }

    /** The host panel is display-toggled by routing; recheck the size on show. */
    invalidateSize() {
        this.map.invalidateSize();
    }

    setRegions(features: RegionFeature[]) {
        this.regions = indexRegions(features);
        this.regionsById.clear();
        for (const f of this.regions) this.regionsById.set(f.properties.id, f);
    }

    regionName(id: string): string {
        return this.regionsById.get(id)?.properties.name ?? id;
    }

    fitRegion(id: string) {
        const f = this.regionsById.get(id);
        if (!f) return;
        const [minx, miny, maxx, maxy] = f._bbox;
        this.map.fitBounds(
            [
                [miny, minx],
                [maxy, maxx],
            ],
            { maxZoom: 8 },
        );
    }

    toggleRegion(id: string) {
        if (this.selected.has(id)) this.selected.delete(id);
        else this.selected.add(id);
        this.renderHighlights();
        this.clearPreview(); // selection changed under the cursor; recompute on move
        this.cb.onSelectionChange([...this.selected]);
    }

    /** Restore a persisted selection (ids unknown to the index are dropped). */
    setSelection(ids: string[]) {
        this.selected.clear();
        for (const id of ids) {
            if (this.regionsById.has(id)) this.selected.add(id);
        }
        this.renderHighlights();
        this.cb.onSelectionChange([...this.selected]);
    }

    coveringRegions(bbox: Bbox): IndexedRegion[] {
        return regionsForBbox(this.regions, bbox);
    }

    bboxSummary(bbox: Bbox): string {
        return bboxAreaKm2(...bbox);
    }

    // --- region picking ---

    private onClick(e: L.LeafletMouseEvent) {
        if (this.bboxMode) return;
        const { lng, lat } = e.latlng;
        const hits = this.regions
            .filter((f) => featureContains(f, lng, lat))
            .sort((a, b) => a._area - b._area); // most specific (smallest) first
        if (hits.length === 0) return;
        this.showRegionPopup(e.latlng, hits);
    }

    private showRegionPopup(latlng: L.LatLng, hits: IndexedRegion[]) {
        this.popupOpen = true;
        this.clearPreview();
        const div = document.createElement("div");
        div.className = "region-popup";
        for (const f of hits) {
            const id = f.properties.id;
            const btn = document.createElement("button");
            btn.textContent = f.properties.name;
            if (this.selected.has(id)) btn.classList.add("selected");
            // Lock the preview to the hovered option, not the raw cursor.
            btn.addEventListener("mouseenter", () => this.setPreviewFeature(f));
            btn.onclick = () => {
                this.toggleRegion(id);
                btn.classList.toggle("selected");
            };
            div.appendChild(btn);
        }
        L.popup({ closeButton: true }).setLatLng(latlng).setContent(div).openOn(this.map);
        this.setPreviewFeature(hits[0]); // default to the smallest (top) option
    }

    private renderHighlights() {
        if (this.highlightLayer) this.map.removeLayer(this.highlightLayer);
        const feats = [...this.selected]
            .map((id) => this.regionsById.get(id))
            .filter((f): f is IndexedRegion => Boolean(f));
        this.highlightLayer = L.geoJSON(feats as never, { style: SELECT_STYLE }).addTo(this.map);
    }

    // --- hover preview ---

    private clearPreview() {
        this.previewId = null;
        if (this.previewLayer) {
            this.map.removeLayer(this.previewLayer);
            this.previewLayer = null;
        }
        if (this.map.hasLayer(this.previewTip)) this.map.removeLayer(this.previewTip);
    }

    private setPreviewFeature(f: IndexedRegion | null, latlng?: L.LatLng) {
        const id = f ? f.properties.id : null;
        if (id === this.previewId && !latlng) return;
        if (this.previewLayer) {
            this.map.removeLayer(this.previewLayer);
            this.previewLayer = null;
        }
        this.previewId = id;
        if (f && id) {
            this.previewLayer = L.geoJSON(f as never, { style: PREVIEW_STYLE }).addTo(this.map);
            if (latlng) this.previewTip.setContent(f.properties.name).setLatLng(latlng).addTo(this.map);
            else if (this.map.hasLayer(this.previewTip)) this.map.removeLayer(this.previewTip);
        } else if (this.map.hasLayer(this.previewTip)) {
            this.map.removeLayer(this.previewTip);
        }
    }

    private onMouseMove(e: L.LeafletMouseEvent) {
        if (this.drawArmed && this.bboxDrawing && this.bboxRect && this.bboxStart) {
            this.bboxRect.setBounds(L.latLngBounds(this.bboxStart, e.latlng));
            return;
        }
        if (this.bboxMode || this.popupOpen) return;
        this.pendingLatLng = e.latlng;
        if (this.map.hasLayer(this.previewTip)) this.previewTip.setLatLng(e.latlng);
        if (!this.rafPending) {
            this.rafPending = true;
            requestAnimationFrame(() => this.updatePreview());
        }
    }

    private updatePreview() {
        this.rafPending = false;
        if (this.popupOpen || !this.pendingLatLng) return;
        const f = smallestRegionAt(this.regions, this.pendingLatLng.lng, this.pendingLatLng.lat);
        // Don't preview a region that's already selected (it's shown in green).
        const region = f && !this.selected.has(f.properties.id) ? f : null;
        this.setPreviewFeature(region, region ? this.pendingLatLng : undefined);
    }

    // --- bounding-box mode ---

    setMode(mode: "regions" | "bbox") {
        const toBbox = mode === "bbox";
        if (toBbox === this.bboxMode) return;
        this.bboxMode = toBbox;
        if (toBbox) {
            this.clearPreview();
        } else {
            this.cancelDraw();
            this.clearBbox();
        }
    }

    armDraw() {
        this.drawArmed = true;
        this.removeHandles(); // old handles must not intercept the new draw
        this.map.dragging.disable();
        this.map.getContainer().classList.add("bbox-cursor");
        this.cb.onDrawStateChange(true);
    }

    cancelDraw() {
        if (!this.drawArmed) return;
        this.drawArmed = false;
        this.map.dragging.enable();
        this.map.getContainer().classList.remove("bbox-cursor");
        if (this.bboxBounds && this.bboxRect) this.buildHandles(); // restore on the kept box
        this.cb.onDrawStateChange(false);
    }

    clearBbox() {
        this.bboxDrawing = false;
        this.removeBox();
        this.cb.onBboxChange(null);
    }

    /** Set the box programmatically (coordinate inputs, restore, use-view). */
    setBbox(b: Bbox, fit = false) {
        this.bboxBounds = L.latLngBounds([b[1], b[0]], [b[3], b[2]]);
        if (this.bboxRect) {
            this.bboxRect.setBounds(this.bboxBounds);
        } else {
            this.bboxRect = L.rectangle(this.bboxBounds, BBOX_STYLE).addTo(this.map);
            this.enableBoxDrag();
        }
        this.buildHandles();
        if (fit) this.map.fitBounds(this.bboxBounds.pad(0.3));
        this.cb.onBboxChange(this.currentBbox());
    }

    /** Box = the current viewport, inset a little so it reads as a box. */
    useCurrentView() {
        const b = this.map.getBounds().pad(-0.12);
        this.setBbox([b.getWest(), b.getSouth(), b.getEast(), b.getNorth()]);
    }

    private onKey(e: KeyboardEvent) {
        if (!this.bboxMode) return;
        if (e.key === "Escape") {
            if (this.drawArmed || this.bboxDrawing) {
                this.bboxDrawing = false;
                this.removeBox();
                this.cancelDraw();
                this.cb.onBboxChange(null);
            }
            return;
        }
        if (!this.bboxBounds || !e.key.startsWith("Arrow")) return;
        e.preventDefault();
        const px = 6;
        const dx = e.key === "ArrowLeft" ? -px : e.key === "ArrowRight" ? px : 0;
        const dy = e.key === "ArrowUp" ? -px : e.key === "ArrowDown" ? px : 0;
        const nw = this.map.latLngToContainerPoint(this.bboxBounds.getNorthWest());
        const se = this.map.latLngToContainerPoint(this.bboxBounds.getSouthEast());
        if (e.shiftKey) {
            // Resize: the south-east corner follows the arrows.
            this.bboxBounds = L.latLngBounds(
                this.map.containerPointToLatLng(nw),
                this.map.containerPointToLatLng(L.point(se.x + dx, se.y + dy)),
            );
        } else {
            this.bboxBounds = L.latLngBounds(
                this.map.containerPointToLatLng(L.point(nw.x + dx, nw.y + dy)),
                this.map.containerPointToLatLng(L.point(se.x + dx, se.y + dy)),
            );
        }
        this.bboxRect?.setBounds(this.bboxBounds);
        this.positionHandles();
        this.cb.onBboxChange(this.currentBbox());
    }

    private currentBbox(): Bbox | null {
        if (!this.bboxBounds) return null;
        return [
            this.bboxBounds.getWest(),
            this.bboxBounds.getSouth(),
            this.bboxBounds.getEast(),
            this.bboxBounds.getNorth(),
        ];
    }

    private onDrawStart(e: L.LeafletMouseEvent) {
        if (!this.drawArmed) return;
        this.bboxDrawing = true;
        this.bboxStart = e.latlng;
        this.removeBox();
        this.bboxRect = L.rectangle(L.latLngBounds(e.latlng, e.latlng), BBOX_STYLE).addTo(this.map);
    }

    private onDrawEnd() {
        if (!this.drawArmed || !this.bboxDrawing) return;
        this.bboxDrawing = false;
        this.finishDraw();
    }

    private finishDraw() {
        this.drawArmed = false;
        this.map.dragging.enable();
        this.map.getContainer().classList.remove("bbox-cursor");
        this.cb.onDrawStateChange(false);
        if (!this.bboxRect) return;
        const b = this.bboxRect.getBounds();
        // Ignore a stray click or micro-drag (no real area to build).
        const a = this.map.latLngToContainerPoint(b.getNorthWest());
        const c = this.map.latLngToContainerPoint(b.getSouthEast());
        if (Math.abs(a.x - c.x) < 5 || Math.abs(a.y - c.y) < 5) {
            this.removeBox();
            this.cb.onBboxChange(null);
            return;
        }
        this.bboxBounds = b;
        this.buildHandles();
        this.enableBoxDrag();
        this.cb.onBboxChange(this.currentBbox());
    }

    // Four draggable corner markers: dragging one resizes about the opposite
    // corner. Leaflet marker-drag suppresses map panning for the duration.
    private buildHandles() {
        this.removeHandles();
        this.bboxHandles = {};
        for (const key of CORNERS) {
            const m = L.marker(cornerLatLng(this.bboxBounds!, key), {
                draggable: true,
                keyboard: false,
                zIndexOffset: 1000,
                // 16 px hit area + a directional resize cursor per corner.
                icon: L.divIcon({
                    className: `bbox-handle bbox-handle-${key}`,
                    iconSize: [16, 16],
                    iconAnchor: [8, 8],
                }),
            }).addTo(this.map);
            m.on("drag", () => {
                const opp = this.bboxHandles![OPPOSITE[key]]!.getLatLng();
                this.bboxBounds = L.latLngBounds(opp, m.getLatLng());
                this.bboxRect!.setBounds(this.bboxBounds);
                for (const k of CORNERS) {
                    if (k !== key) this.bboxHandles![k]!.setLatLng(cornerLatLng(this.bboxBounds, k));
                }
                this.cb.onBboxChange(this.currentBbox());
            });
            m.on("dragend", () => {
                this.positionHandles();
                this.cb.onBboxChange(this.currentBbox());
            });
            this.bboxHandles[key] = m;
        }
    }

    private positionHandles() {
        if (!this.bboxHandles || !this.bboxBounds) return;
        for (const key of CORNERS) this.bboxHandles[key]!.setLatLng(cornerLatLng(this.bboxBounds, key));
    }

    private removeHandles() {
        if (!this.bboxHandles) return;
        for (const key of CORNERS) {
            const m = this.bboxHandles[key];
            if (m) this.map.removeLayer(m);
        }
        this.bboxHandles = null;
    }

    private removeBox() {
        this.removeHandles();
        if (this.bboxRect) {
            this.map.removeLayer(this.bboxRect);
            this.bboxRect = null;
        }
        this.bboxBounds = null;
    }

    // Drag the box body to move the whole box: swallow the mousedown so the map
    // doesn't pan underneath, translate the bounds by the cursor delta.
    private enableBoxDrag() {
        this.bboxRect!.on("mousedown", (e) => {
            if (this.drawArmed) return; // a redraw is in progress
            L.DomEvent.stop(e.originalEvent);
            this.map.dragging.disable();
            let last = e.latlng;
            const onMove = (ev: L.LeafletMouseEvent) => {
                const dLat = ev.latlng.lat - last.lat;
                const dLng = ev.latlng.lng - last.lng;
                last = ev.latlng;
                this.bboxBounds = L.latLngBounds(
                    [this.bboxBounds!.getSouth() + dLat, this.bboxBounds!.getWest() + dLng],
                    [this.bboxBounds!.getNorth() + dLat, this.bboxBounds!.getEast() + dLng],
                );
                this.bboxRect!.setBounds(this.bboxBounds);
                this.positionHandles();
                this.cb.onBboxChange(this.currentBbox());
            };
            const onUp = () => {
                this.map.off("mousemove", onMove);
                this.map.off("mouseup", onUp);
                this.map.dragging.enable();
                this.cb.onBboxChange(this.currentBbox());
            };
            this.map.on("mousemove", onMove);
            this.map.on("mouseup", onUp);
        });
    }
}
