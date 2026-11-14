// The coverage flow's one store: the selection, its resolution, and the ledger,
// held reactively for the map pane and the steps column to share.
//
// The arithmetic all lives in `lib/catalog/` — this class owns *state and
// lifetime*. Two `SelectionResolver`s, deliberately: each prunes cache entries
// that were not part of its latest answer, so one resolver alternating between
// the committed selection and the box being dragged would evict the committed
// answers on every move.

import type { ArticleIndexDocument } from "../catalog/articles";
import { CatalogClient } from "../catalog/client";
import { cellsIntersecting, coverageBbox, parseCellId, type UBox } from "../core/grid";
import { cellsTouchingHoles, detailBandId } from "./shape";
import { ledgerFor, type Ledger } from "../catalog/ledger";
import type { Catalog, RegionEntry, SkinEntry } from "../catalog/manifest";
import type { CellIndexDocument, RegionCellsDocument, TerrainIndexDocument } from "../catalog/satellites";
import {
    browserSkinStorage,
    loadCustomSkins,
    persistCustomSkins,
    prepareCustomSkin,
    validateCustomSkin,
    type CustomSkinRecord,
    type SkinStorage,
} from "../skin/custom";
import {
    emptySelection,
    SelectionResolver,
    withoutPart,
    withPart,
    type Selection,
    type SelectionContext,
    type SelectionResolution,
} from "../catalog/selection";

/** A boundary's bbox area in µdeg², the ladder's smallest-first sort key. Not a
 *  real area — but ordering nested admin regions only needs monotonicity, and a
 *  child's bbox never out-spans its parent's. */
function ringsSpan(rings: [number, number][][]): number {
    let minLat = Infinity;
    let maxLat = -Infinity;
    let minLon = Infinity;
    let maxLon = -Infinity;
    for (const ring of rings) {
        for (const [lat, lon] of ring) {
            if (lat < minLat) minLat = lat;
            if (lat > maxLat) maxLat = lat;
            if (lon < minLon) minLon = lon;
            if (lon > maxLon) maxLon = lon;
        }
    }
    return (maxLat - minLat) * (maxLon - minLon);
}

function pointInRings(lat: number, lon: number, rings: [number, number][][]): boolean {
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

export class CoverageStore {
    readonly client: CatalogClient;
    readonly catalog: Catalog;
    /** The root document verbatim — the assembly engine takes it as the schema
     *  document, and re-serialising a parsed copy would hand it different bytes
     *  than the ones the catalog's digests admitted. */
    readonly rootBody: string;

    /** Every band's cell index, once loaded. Null while loading — a price
     *  computed from some bands is exactly the not-final state the ledger's
     *  `isFinal` exists to name, so nothing is priced until all arrive. */
    indices = $state<ReadonlyMap<string, CellIndexDocument> | null>(null);
    /** The pinned terrain index, or `null` — which is both "not loaded yet" and
     *  "this catalog publishes no raster", because a consumer treats the two the
     *  same way: no elevation, no refusal. */
    terrain = $state<TerrainIndexDocument | null>(null);
    articles = $state<ArticleIndexDocument | null>(null);
    indexError = $state<string | null>(null);

    /** Region cell lists, as they arrive. Replaced wholesale on each arrival so
     *  a `$derived` over the map re-fires. */
    regionCells = $state<ReadonlyMap<string, RegionCellsDocument>>(new Map());
    /** Region list fetches that failed, by region id — the part row says so. */
    regionErrors = $state<ReadonlyMap<string, string>>(new Map());

    selection = $state<Selection>(emptySelection());
    lightSkinId = $state<string>("default");
    darkSkinId = $state<string>("dusk");
    customSkinRecords = $state<CustomSkinRecord[]>([]);

    /** Somewhere the map should fly to — a warning row was clicked. Consumed
     *  (nulled) by the map after the flight. */
    focus = $state<UBox | null>(null);

    /** A part row under the pointer; the map thickens its outline. */
    highlightPartId = $state<string | null>(null);

    private readonly resolver = new SelectionResolver();
    /** Prices the box being dragged, apart from the committed resolver so a drag
     *  never evicts the committed selection's warm cache. */
    private readonly dragResolver = new SelectionResolver();
    private readonly skinStorage: SkinStorage | null;
    private nextPartId = 1;
    private boxCount = 0;

    constructor(client: CatalogClient, rootBody: string, skinStorage: SkinStorage | null = browserSkinStorage()) {
        this.client = client;
        this.catalog = client.catalog;
        this.rootBody = rootBody;
        this.skinStorage = skinStorage;
        this.customSkinRecords = loadCustomSkins(skinStorage, client.catalog.schema);
        void this.loadIndices();
    }

    /** Carry user intent into one new catalog, with no old satellite or resolver cache. */
    refreshed(client: CatalogClient, rootBody: string): CoverageStore {
        const next = new CoverageStore(client, rootBody, this.skinStorage);
        next.selection = this.selection;
        next.nextPartId = this.nextPartId;
        next.boxCount = this.boxCount;
        for (const part of next.selection.parts) {
            if (part.kind !== "region") continue;
            if (next.region(part.regionId)) void next.fetchRegionCells(part.regionId);
            else next.regionErrors = new Map(next.regionErrors).set(part.regionId, "This region is no longer in the map catalog. Remove it or choose another region.");
        }
        const carried = this.customSkinRecords.filter((record) =>
            !next.customSkinRecords.some((stored) => stored.skin.id === record.skin.id) &&
            validateCustomSkin(record.skin, next.catalog.schema),
        );
        if (carried.length) {
            const records = [...next.customSkinRecords, ...carried];
            persistCustomSkins(next.skinStorage, next.catalog.schema, records);
            next.customSkinRecords = records;
        }
        if (next.lightSkins.some((skin) => skin.id === this.lightSkinId)) next.lightSkinId = this.lightSkinId;
        if (next.darkSkins.some((skin) => skin.id === this.darkSkinId)) next.darkSkinId = this.darkSkinId;
        if (next.lightSkinId !== this.lightSkinId || next.darkSkinId !== this.darkSkinId) {
            next.refreshNotice = "A previous map style is unavailable. Check the new map style before downloading.";
        }
        return next;
    }

    refreshNotice = $state<string | null>(null);

    /** Resolver cache effectiveness, for tests and devtools. Not rendered. */
    get resolverStats(): { computed: number; reused: number } {
        return this.resolver.stats;
    }

    private async loadIndices(): Promise<void> {
        try {
            // Accept all indices together so the download total is complete.
            const [indices, terrain, articles] = await Promise.all([
                this.client.cellIndices(), this.client.terrain(), this.client.articles(),
            ]);
            this.articles = articles;
            this.terrain = terrain;
            this.indices = indices;
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
        return { catalog: this.catalog, indices: this.indices, regionCells: this.regionCells, terrain: this.terrain };
    }

    /**
     * The committed selection, resolved — or the sentence saying why it cannot
     * be. A resolution *throws* for exactly two kinds of reason, and both are
     * refusals rather than crashes: a broken publish (a region list naming a cell
     * no band index carries), and a part outside what the grid enumerates (a box
     * over half the planet). The UI shows
     * the sentence and the parts list stays alive, so the offending part can be
     * removed.
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
        return resolution && this.indices ? ledgerFor(resolution, this.catalog, this.indices, this.articles) : null;
    });

    readonly skins = $derived.by<SkinEntry[]>(() => [
        ...this.catalog.skins,
        ...this.customSkinRecords.map((record) => record.skin),
    ]);

    readonly lightSkins = $derived.by<SkinEntry[]>(() =>
        this.skins.filter((skin) => skin.id === "default" || this.basedOn(skin.id) === "default"),
    );

    readonly darkSkins = $derived.by<SkinEntry[]>(() =>
        this.skins.filter((skin) => skin.id === "dusk" || this.basedOn(skin.id) === "dusk"),
    );

    readonly lightSkin = $derived.by<SkinEntry>(() => {
        const skin = this.lightSkins.find((candidate) => candidate.id === this.lightSkinId) ?? this.lightSkins[0];
        if (!skin) throw new Error('the catalog has no "default" Light skin');
        return skin;
    });

    readonly darkSkin = $derived.by<SkinEntry>(() => {
        const skin = this.darkSkins.find((candidate) => candidate.id === this.darkSkinId) ?? this.darkSkins[0];
        if (!skin) throw new Error('the catalog has no "dusk" Dark skin');
        return skin;
    });

    private basedOn(id: string): string | null {
        return this.customSkinRecords.find((record) => record.skin.id === id)?.based_on ?? null;
    }

    saveCustomSkin(draft: SkinEntry, name: string, basedOn: string): SkinEntry {
        const existing = this.customSkinRecords.find((record) => record.skin.id === draft.id) ?? null;
        const source = existing?.based_on ?? basedOn;
        if (source !== "default" && source !== "dusk") {
            throw new Error(`custom skin source ${JSON.stringify(source)} is not Light or Dark`);
        }
        const skin = prepareCustomSkin(draft, this.catalog.schema, name, existing?.skin ?? null);
        const record: CustomSkinRecord = {
            skin,
            based_on: source,
        };
        const next = existing
            ? this.customSkinRecords.map((candidate) => (candidate.skin.id === skin.id ? record : candidate))
            : [...this.customSkinRecords, record];
        // Persist first: a denied/quota-full browser must not render a "saved"
        // skin that disappears on refresh.
        persistCustomSkins(this.skinStorage, this.catalog.schema, next);
        this.customSkinRecords = next;
        if (record.based_on === "default") this.lightSkinId = skin.id;
        else if (record.based_on === "dusk") this.darkSkinId = skin.id;
        return skin;
    }

    deleteCustomSkin(id: string): void {
        const next = this.customSkinRecords.filter((record) => record.skin.id !== id);
        if (next.length === this.customSkinRecords.length) return;
        persistCustomSkins(this.skinStorage, this.catalog.schema, next);
        this.customSkinRecords = next;
        if (this.lightSkinId === id) this.lightSkinId = "default";
        if (this.darkSkinId === id) this.darkSkinId = "dusk";
    }

    /**
     * The selection's holes, from **every** band, deduplicated.
     *
     * A hole is ground with no published cell, and it is real in whichever band
     * it occurs: a missing mid or coarse cell means the assembled map has no
     * zoomed-out context there. Only the partial hatch below is band-restricted.
     * The dedup is by canonical id, which also collapses the network band's
     * squares onto the fine band's where both miss.
     *
     * `acceptHoles` must be derived from this: the assembly may only be told to
     * accept holes the UI has actually shown, and this is the set it shows.
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

    /** Partial cells in the detail band; coarse context never warns. */
    partialDetailCells(): string[] {
        const coverage = this.ledger?.coverage;
        if (!coverage) return [];
        return coverage.partialDetailByBand.get(detailBandId(this.catalog)) ?? [];
    }

    /** Warn on partial detail cells next to a hole. Region-border overhang alone
     *  does not establish a missing-data gap inside the selected region. */
    partialHatchCells(): string[] {
        const detailHoles =
            this.ledger?.coverage.holesByBand.get(detailBandId(this.catalog)) ?? [];
        return cellsTouchingHoles(this.partialDetailCells(), detailHoles);
    }

    /** Focus the same missing ground that the summary warns about and the map hatches. */
    focusWarnings(kind: "hole" | "partial"): void {
        const ids = kind === "hole" ? this.holeCells() : this.partialHatchCells();
        const box = coverageBbox(ids.map((id) => parseCellId(id)));
        if (box) this.focus = box;
    }

    region(regionId: string): RegionEntry | undefined {
        return this.catalog.regions.find((r) => r.id === regionId);
    }

    /**
     * Every catalog region whose boundary contains the point, smallest first —
     * the ancestor ladder a map click offers. Containment is against the drawable
     * rings; the added part still resolves through the region's stored cell list.
     */
    regionsAt(lat: number, lon: number): RegionEntry[] {
        return this.catalog.regions
            .filter((region) => pointInRings(lat, lon, region.boundary.rings))
            .sort((a, b) => ringsSpan(a.boundary.rings) - ringsSpan(b.boundary.rings));
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
    drawError = $state<string | null>(null);

    /**
     * Price a box mid-drag — the live chip under the rubber band.
     *
     * Through the same resolver and ledger arithmetic as everything else, so the
     * number under the cursor *is* `ledgerFor`'s number by construction, rather
     * than a second pricing path that can drift.
     */
    priceDraggedBox(box: UBox): { bytes: number; cells: number } | { refused: true } | null {
        const ctx = this.ctx;
        if (!ctx || !this.indices) return null;
        const candidate: Selection = { parts: [{ kind: "box", id: "drag", name: "", box }] };
        try {
            const ledger = ledgerFor(this.dragResolver.resolve(candidate, ctx), this.catalog, this.indices, this.articles);
            return { bytes: ledger.totalBytes, cells: ledger.cellCount };
        } catch {
            // The grid refused to enumerate it — same refusal the add gives.
            return { refused: true };
        }
    }

    addBox(box: UBox): void {
        // Refuse a box the grid cannot enumerate *before* it becomes a part —
        // as a part it would make every resolution throw, taking the whole
        // ledger down with it rather than just this box.
        try {
            for (const band of this.catalog.schema.bands) cellsIntersecting(band.cell_log2, box);
        } catch {
            this.drawError = "That box covers more cells than a map can hold — draw a smaller one, or add a whole region instead.";
            return;
        }
        this.drawError = null;
        this.boxCount += 1;
        const midLat = (box.minLat + box.maxLat) / 2;
        const midLon = (box.minLon + box.maxLon) / 2;
        this.selection = withPart(this.selection, {
            kind: "box",
            id: `part-${this.nextPartId++}`,
            name: this.areaName("Box", midLat, midLon, this.boxCount),
            box,
        });
    }

    /** "Box — <the smallest catalog region under its centre>", because "Box 3"
     *  tells nobody which box to remove. Containment is tested against the
     *  region's actual boundary rings, not its bounding box: a box centred in the
     *  sea inside Italy's bbox is not "Box — Italy". Falls back to a counter
     *  off-catalog. */
    private areaName(prefix: string, midLat: number, midLon: number, count: number): string {
        const best = this.regionsAt(midLat, midLon)[0];
        return best ? `${prefix} — ${best.name}` : `${prefix} ${count}`;
    }

    /** The regions and the boxes of the selection, and the ids of the two skins, as plain values that a database can store. */
    area(): { regions: string[]; boxes: UBox[]; light: string; dark: string } {
        const parts = this.selection.parts;
        return {
            regions: parts.flatMap((part) => (part.kind === "region" ? [part.regionId] : [])),
            boxes: parts.flatMap((part) => (part.kind === "box" ? [{ ...part.box }] : [])),
            light: this.lightSkin.id,
            dark: this.darkSkin.id,
        };
    }

    /** Replaces the selection and the skins with `area`. A region that the catalog does not hold is left out, and a
     *  skin that is not there gives the first of its kind. */
    setArea(area: { regions: string[]; boxes: UBox[]; light: string; dark: string }): void {
        this.selection = emptySelection();
        for (const regionId of area.regions) this.addRegion(regionId);
        for (const box of area.boxes) this.addBox(box);
        this.lightSkinId = area.light;
        this.darkSkinId = area.dark;
    }

    removePart(partId: string): void {
        this.selection = withoutPart(this.selection, partId);
        if (this.highlightPartId === partId) this.highlightPartId = null;
    }
}
