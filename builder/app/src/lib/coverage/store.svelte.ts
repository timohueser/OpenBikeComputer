// The coverage flow's one store (#1038): the selection, its resolution, and the
// ledger, held reactively for the map pane and the steps column to share.
//
// The arithmetic all lives in `lib/catalog/v2/` — this class owns *state and
// lifetime*: which parts exist, which satellite documents have arrived, which
// skin is picked, and the two `SelectionResolver`s that keep the corridor
// slider smooth. Two resolvers, deliberately: each prunes cache entries that
// were not part of its latest answer, so one resolver alternating between "the
// selection" (the map) and "the selection plus the corridor panel's preview"
// (the adds-line) would evict half its cache on every frame. The main resolver
// answers only the committed selection; the preview resolver answers the
// superset and keeps both warm.

import { CatalogV2Client } from "../catalog/v2/client";
import { cellsIntersecting, coverageBbox, parseCellId, type UBox } from "../catalog/v2/grid";
import { cellsTouchingHoles, detailBandId } from "./shape";
import { ledgerFor, type Ledger } from "../catalog/v2/ledger";
import type { CatalogV2, RegionEntry, SkinEntry } from "../catalog/v2/manifest";
import type { CellIndexDocument, RegionCellsDocument } from "../catalog/v2/satellites";
import {
    emptySelection,
    SelectionResolver,
    withCorridorRadius,
    withoutPart,
    withPart,
    type CorridorPart,
    type LatLon,
    type Selection,
    type SelectionContext,
    type SelectionResolution,
} from "../catalog/v2/selection";

/** The slider's range and default, metres. The default is a day-ride's "what if
 *  I take the other valley" allowance — wide enough that a detour stays on the
 *  map, narrow enough that a corridor stays corridor-shaped (cells are ~29 km,
 *  so the first step past the route is already generous). */
export const CORRIDOR_RADIUS_DEFAULT_M = 10_000;
export const CORRIDOR_RADIUS_MIN_M = 2_000;
export const CORRIDOR_RADIUS_MAX_M = 50_000;

/** What the corridor panel's "adds …" line states about a candidate. */
export interface PreviewSummary {
    /** Bytes the map grows by — the candidate's cells minus what the selection
     *  already covers. */
    addsBytes: number;
    addsCells: number;
    /** Disjoint patches the candidate itself forms (2 routes far apart = 2). */
    patches: number;
}

export class CoverageStore {
    readonly client: CatalogV2Client;
    readonly catalog: CatalogV2;
    /** The root document verbatim — the assembly engine takes it as the schema
     *  document, and re-serialising a parsed copy would hand it different bytes
     *  than the ones the catalog's digests admitted. */
    readonly rootBody: string;

    /** Every band's cell index, once loaded. Null while loading — a price
     *  computed from some bands is exactly the not-final state the ledger's
     *  `isFinal` exists to name, so nothing is priced until all arrive. */
    indices = $state<ReadonlyMap<string, CellIndexDocument> | null>(null);
    indexError = $state<string | null>(null);

    /** Region cell lists, as they arrive. Replaced wholesale on each arrival so
     *  a `$derived` over the map re-fires. */
    regionCells = $state<ReadonlyMap<string, RegionCellsDocument>>(new Map());
    /** Region list fetches that failed, by region id — the part row says so. */
    regionErrors = $state<ReadonlyMap<string, string>>(new Map());

    selection = $state<Selection>(emptySelection(CORRIDOR_RADIUS_DEFAULT_M));
    skinId = $state<string>("");

    /** The corridor panel's checked-but-not-yet-added routes, drawn dashed on
     *  the map and priced by {@link previewSummary}. */
    previewParts = $state<CorridorPart[]>([]);

    /** Somewhere the map should fly to — a warning row was clicked. Consumed
     *  (nulled) by the map after the flight. */
    focus = $state<UBox | null>(null);

    /** A part row under the pointer; the map thickens its outline. */
    highlightPartId = $state<string | null>(null);

    private readonly resolver = new SelectionResolver();
    private readonly previewResolver = new SelectionResolver();
    private nextPartId = 1;
    private boxCount = 0;

    constructor(client: CatalogV2Client, rootBody: string) {
        this.client = client;
        this.catalog = client.catalog;
        this.rootBody = rootBody;
        this.skinId = client.catalog.skins[0].id;
        void this.loadIndices();
    }

    /** Resolver cache effectiveness, for the perf note in tests and devtools.
     *  Not rendered. */
    get resolverStats(): { computed: number; reused: number } {
        return this.resolver.stats;
    }

    private async loadIndices(): Promise<void> {
        try {
            this.indices = await this.client.cellIndices();
        } catch (e) {
            this.indexError = e instanceof Error ? e.message : String(e);
        }
    }

    /** Retry after a failed index load. */
    reloadIndices(): void {
        this.indexError = null;
        void this.loadIndices();
    }

    private get ctx(): SelectionContext | null {
        if (!this.indices) return null;
        return { catalog: this.catalog, indices: this.indices, regionCells: this.regionCells };
    }

    /**
     * The committed selection, resolved — or the sentence saying why it cannot
     * be. A resolution *throws* for exactly two kinds of reason, and both are
     * refusals rather than crashes: a broken publish (a region list naming a
     * cell no band index carries — `assertRegionCellsIndexed`, the #1030 rule
     * that a named cell with no bytes must never be drawn as a legal hole), and
     * a part outside what the grid enumerates (a box over half the planet, a
     * corridor crossing the antimeridian). The UI shows the sentence and the
     * parts list stays alive, so the offending part can be removed.
     */
    readonly resolved = $derived.by<{ resolution: SelectionResolution | null; error: string | null }>(() => {
        const ctx = this.ctx;
        if (!ctx) return { resolution: null, error: null };
        try {
            return { resolution: this.resolver.resolve(this.selection, ctx), error: null };
        } catch (e) {
            return { resolution: null, error: e instanceof Error ? e.message : String(e) };
        }
    });

    get resolution(): SelectionResolution | null {
        return this.resolved.resolution;
    }

    get resolutionError(): string | null {
        return this.resolved.error;
    }

    readonly ledger = $derived.by<Ledger | null>(() => {
        const resolution = this.resolved.resolution;
        return resolution && this.indices ? ledgerFor(resolution, this.catalog, this.indices) : null;
    });

    /** The selection plus the panel's preview routes — what the map draws while
     *  the panel is open, and what the adds-line is priced from. Null (never a
     *  throw) when a preview route cannot be resolved; the panel says so. */
    readonly previewed = $derived.by<{ resolution: SelectionResolution | null; error: string | null }>(() => {
        const ctx = this.ctx;
        if (!ctx || this.previewParts.length === 0) return { resolution: null, error: null };
        let candidate = this.selection;
        for (const part of this.previewParts) candidate = withPart(candidate, part);
        try {
            return { resolution: this.previewResolver.resolve(candidate, ctx), error: null };
        } catch (e) {
            return { resolution: null, error: e instanceof Error ? e.message : String(e) };
        }
    });

    get previewResolution(): SelectionResolution | null {
        return this.previewed.resolution;
    }

    readonly skin = $derived.by<SkinEntry>(() => {
        return this.catalog.skins.find((s) => s.id === this.skinId) ?? this.catalog.skins[0];
    });

    /** What the panel's candidate would add to the committed map. */
    previewSummary(patches: number): PreviewSummary | null {
        const withPreview = this.previewResolution;
        const ledger = this.ledger;
        if (!withPreview || !ledger || !this.indices) return null;
        const candidate = ledgerFor(withPreview, this.catalog, this.indices);
        return {
            addsBytes: Math.max(0, candidate.totalBytes - ledger.totalBytes),
            addsCells: Math.max(0, candidate.cellCount - ledger.cellCount),
            patches,
        };
    }

    /**
     * The selection's holes, from **every** band, deduplicated (#1041 A5).
     *
     * A hole is ground with no published cell, and it is real in whichever
     * band it occurs: a missing mid or coarse cell means the assembled map has
     * no zoomed-out context there, which is exactly the kind of surprise the
     * hatch exists to show *before* the download. So holes are counted,
     * sentenced and hatched from every band — only the partial hatch below is
     * band-restricted. The dedup is by canonical id, which also collapses the
     * network band's squares onto the fine band's where both miss (they share
     * a cell size by design).
     *
     * This selector is also what `acceptHoles` must be derived from: the
     * assembly may only be told to accept holes the UI has actually shown, and
     * this is the set it shows.
     */
    holeCells(): string[] {
        const coverage = this.ledger?.coverage;
        if (!coverage) return [];
        const seen = new Set<string>();
        for (const ids of coverage.holesByBand.values()) {
            for (const id of ids) seen.add(id);
        }
        return [...seen].sort();
    }

    /**
     * Partial cells in the **detail band** — the warning sentence's full count.
     *
     * Detail band only, unlike the holes: the bands overlap on the ground (the
     * mid band's squares each cover four of the fine band's), so a cross-band
     * partial count would count the same ground two or three times, and the
     * detail band is the band the outline is drawn from and street detail
     * lives in. The coarse context band never appears here at all — the
     * ledger's `partialDetailByBand` keeps it out (#1025's rule).
     */
    partialDetailCells(): string[] {
        const coverage = this.ledger?.coverage;
        if (!coverage) return [];
        return coverage.partialDetailByBand.get(detailBandId(this.catalog)) ?? [];
    }

    /**
     * The partial detail cells that **hatch** (#1041 A9, decided): only those
     * abutting a hole in the same band. At extract scale most fine-band
     * partials are border-overhang normality, and hatching a curated pick's
     * whole border would be §8 U1's rejected noise tax through another door —
     * where a partial cell meets a hole, though, the detail visibly stops, and
     * that edge is worth the ink. The warning sentence keeps the full count
     * ({@link partialDetailCells}); this set is what the map draws.
     */
    partialHatchCells(): string[] {
        const detailHoles =
            this.ledger?.coverage.holesByBand.get(detailBandId(this.catalog)) ?? [];
        return cellsTouchingHoles(this.partialDetailCells(), detailHoles);
    }

    /**
     * Point the map at the selection's warned ground — the ledger's warning
     * line and the map's hatched patches are the same fact in two places, and
     * clicking either zooms to it (mock R2·1 interaction note). For partials
     * that is the *hatched* subset: with nothing hatched there is nothing on
     * the map to fly to, and the summary renders the sentence unclickable.
     */
    focusWarnings(kind: "hole" | "partial"): void {
        const ids = kind === "hole" ? this.holeCells() : this.partialHatchCells();
        const box = coverageBbox(ids.map((id) => parseCellId(id)));
        if (box) this.focus = box;
    }

    // --- parts -----------------------------------------------------------

    region(regionId: string): RegionEntry | undefined {
        return this.catalog.regions.find((r) => r.id === regionId);
    }

    /** Whether a region is already a part — the popover marks it, and a second
     *  add is a no-op rather than a duplicate row pricing the same cells. */
    hasRegion(regionId: string): boolean {
        return this.selection.parts.some((p) => p.kind === "region" && p.regionId === regionId);
    }

    addRegion(regionId: string): void {
        const entry = this.region(regionId);
        if (!entry || this.hasRegion(regionId)) return;
        this.selection = withPart(this.selection, {
            kind: "region",
            id: `part-${this.nextPartId++}`,
            name: entry.name,
            regionId,
        });
        void this.fetchRegionCells(regionId);
    }

    private async fetchRegionCells(regionId: string): Promise<void> {
        if (this.regionCells.has(regionId)) return;
        try {
            const doc = await this.client.regionCellList(regionId);
            this.regionCells = new Map(this.regionCells).set(regionId, doc);
            if (this.regionErrors.has(regionId)) {
                const errors = new Map(this.regionErrors);
                errors.delete(regionId);
                this.regionErrors = errors;
            }
        } catch (e) {
            this.regionErrors = new Map(this.regionErrors).set(
                regionId,
                e instanceof Error ? e.message : String(e),
            );
        }
    }

    /** Retry a region whose cell list failed to arrive. */
    retryRegion(regionId: string): void {
        void this.fetchRegionCells(regionId);
    }

    /** Why the last drawn box was refused, shown as a map chip until the next
     *  draw. */
    boxError = $state<string | null>(null);

    addBox(box: UBox): void {
        // Refuse a box the grid cannot enumerate *before* it becomes a part —
        // as a part it would make every resolution throw, taking the whole
        // ledger down with it rather than just this box.
        try {
            for (const band of this.catalog.schema.bands) cellsIntersecting(band.cell_log2, box);
        } catch {
            this.boxError = "That box covers more cells than a map can hold — draw a smaller one, or add a whole region instead.";
            return;
        }
        this.boxError = null;
        this.boxCount += 1;
        this.selection = withPart(this.selection, {
            kind: "box",
            id: `part-${this.nextPartId++}`,
            name: this.boxName(box),
            box,
        });
    }

    /** "Box — <the smallest catalog region under its centre>", because "Box 3"
     *  tells nobody which box to remove. Falls back to a counter off-catalog. */
    private boxName(box: UBox): string {
        const midLat = (box.minLat + box.maxLat) / 2;
        const midLon = (box.minLon + box.maxLon) / 2;
        let best: RegionEntry | null = null;
        let bestSpan = Infinity;
        for (const region of this.catalog.regions) {
            let contains = false;
            let minLat = Infinity;
            let maxLat = -Infinity;
            let minLon = Infinity;
            let maxLon = -Infinity;
            for (const ring of region.boundary.rings) {
                for (const [lat, lon] of ring) {
                    if (lat < minLat) minLat = lat;
                    if (lat > maxLat) maxLat = lat;
                    if (lon < minLon) minLon = lon;
                    if (lon > maxLon) maxLon = lon;
                }
            }
            contains = midLat >= minLat && midLat <= maxLat && midLon >= minLon && midLon <= maxLon;
            const span = (maxLat - minLat) * (maxLon - minLon);
            if (contains && span < bestSpan) {
                best = region;
                bestSpan = span;
            }
        }
        return best ? `Box — ${best.name}` : `Box ${this.boxCount}`;
    }

    addCorridor(name: string, points: LatLon[]): void {
        this.selection = withPart(this.selection, {
            kind: "corridor",
            id: `part-${this.nextPartId++}`,
            name,
            points,
        });
    }

    removePart(partId: string): void {
        this.selection = withoutPart(this.selection, partId);
        if (this.highlightPartId === partId) this.highlightPartId = null;
    }

    setCorridorRadius(radiusM: number): void {
        this.selection = withCorridorRadius(this.selection, radiusM);
    }

    /** A preview part with a stable id, so the panel can replace one route's
     *  entry without perturbing the others' cache keys. */
    makePreviewPart(routeId: string, name: string, points: LatLon[]): CorridorPart {
        return { kind: "corridor", id: `preview-${routeId}`, name, points };
    }

    /** Commit the panel's checked routes: one corridor part per route, because a
     *  part buffers a single polyline and joining two routes into one would
     *  bridge the ground between them with corridor cells — the exact gap the
     *  design promises stays a hole. */
    commitPreview(): void {
        for (const preview of this.previewParts) {
            this.addCorridor(preview.name, preview.points);
        }
        this.previewParts = [];
    }
}
