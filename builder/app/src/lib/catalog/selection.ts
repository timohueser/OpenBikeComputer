// The selection: a list of composable **parts** that resolve to a set of cells
// per band.
//
// This is epic #1016 §8 U2 as data. The map owns selection through a tool rail;
// step 1 becomes a ledger of parts — a picked region, a drawn box, a route
// corridor — each named, each removable, each with its own size. So a selection
// is an ordered list of parts and a resolution is a *union*, and both of those
// are here rather than in a component, because the arithmetic that decides what
// a map contains should be testable without a browser.
//
// Three properties are worth stating because they are consequences rather than
// features:
//
//   * **Generous coarse coverage is not a rule.** Every part resolves through
//     the same "cells whose square intersects this shape" test, run once per
//     band. Because coarse bands use larger cells, that single test yields
//     precise coverage at `2^18` and whole-cell context at `2^20`
//     (`OBCA_Spec.md` §1.2). There is no coarse-band special case anywhere in
//     this file, and there must never be one.
//   * **A region part is a lookup, not a computation.** §11.7's cell list is
//     stored precisely so no consumer derives it from the drawable boundary; a
//     simplification error would drop an edge cell, and a dropped fine cell is a
//     silent hole in street detail. So `region` parts read the published list
//     and nothing else.
//   * **Overlap is free.** Two parts sharing ground share the *same cells*, and
//     the union counts them once. That is the epic's headline saving, and it is
//     also why per-part bytes need two answers (below).

import { corridorCells, type LatLon } from "./corridor";
import { cellsIntersecting, formatCellId, type UBox } from "./grid";
import type { BandEntry, Catalog } from "./manifest";
import {
    assertRegionCellsIndexed,
    cellIndexHas,
    terrainEmptyAt,
    type CellIndexDocument,
    type RegionCellsDocument,
    type TerrainIndexDocument,
} from "./satellites";

export type { LatLon } from "./corridor";

interface PartBase {
    /** Stable within a session; the UI's key for the row and the remove button. */
    id: string;
    /**
     * What the parts list calls it. Held by the part rather than derived,
     * because the three kinds get their names from three different places — the
     * catalog's region name, the user's drawn box, the route's own title — and a
     * list that renamed a row when the catalog reloaded would be a list that
     * cannot be trusted to be the same list.
     */
    name: string;
}

/** A curated region, by its catalog id: its published cell list, verbatim. */
export interface RegionPart extends PartBase {
    kind: "region";
    regionId: string;
}

/** A box drawn on the map, in integer microdegrees. */
export interface BoxPart extends PartBase {
    kind: "box";
    box: UBox;
}

/** One route, buffered by the selection's global corridor radius. */
export interface CorridorPart extends PartBase {
    kind: "corridor";
    points: LatLon[];
}

export type SelectionPart = RegionPart | BoxPart | CorridorPart;

/**
 * A selection: the parts, and the one corridor width that applies to every
 * route in the map.
 *
 * The radius is on the selection rather than on each corridor part because §8 U3
 * decided it is a single global slider. Per-part widths would be a different UI
 * and a different mock; this type is the one that was approved.
 */
export interface Selection {
    parts: SelectionPart[];
    corridorRadiusM: number;
}

/** Everything a resolution needs that is not in the selection itself. */
export interface SelectionContext {
    catalog: Catalog;
    /** Band id → its cell index. A band with no index resolves to nothing and
     *  says so; it does not silently drop out of the map. */
    indices: ReadonlyMap<string, CellIndexDocument>;
    /** Region id → its published cell list, for every `region` part in play. */
    regionCells: ReadonlyMap<string, RegionCellsDocument>;
    /** The pinned terrain index, or `null`/absent when the catalog publishes no
     *  raster — in which case the selection simply names no terrain and the map
     *  assembles exactly as it did before terrain existed (`OBCC_Spec.md` §13). */
    terrain?: TerrainIndexDocument | null;
}

/** One part's contribution, as the parts list shows it. */
export interface PartResolution {
    part: SelectionPart;
    /** Band id → the canonical cell ids this part contributes, sorted. */
    cellsByBand: Map<string, string[]>;
    cellCount: number;
    /**
     * Bytes of the published cells this part covers.
     *
     * **Gross**: a cell shared with another part is counted in both, so these do
     * not sum to the selection's total. That is the honest answer to "how big is
     * this part", and {@link PartResolution.marginalBytes} is the honest answer
     * to the other question a remove button asks.
     */
    bytes: number;
    /** Bytes of the cells **no other part** contributes — what removing this
     *  part would actually save. Sums to at most the selection's total. */
    marginalBytes: number;
    /** Cell ids this part names that the catalog does not publish, per band. */
    missingByBand: Map<string, string[]>;
    missingCount: number;
    /**
     * True while this part's answer is still arriving — a `region` whose stored
     * cell list has not been fetched.
     *
     * It exists because the alternative is indistinguishable from the truth: a
     * pending region contributes no cells, which is exactly what an empty region
     * contributes, so without this flag a summary card shows "0 B" for DACH
     * with the same confidence it shows 0 B for a box drawn over the sea. One
     * of those is a number and the other is a spinner.
     */
    pending: boolean;
}

/** The raster a selection covers (§13.3, EL4). Separate from `cellsByBand`
 *  because terrain is a second artifact class, not a band — and separately
 *  priced, because a rider may take the map without it. */
export interface TerrainResolution {
    /** Squares with a published object: what the download fetches, sorted. */
    cells: string[];
    /** Squares that are canonically void (§13.6): coverage with no object, so
     *  they cost nothing and reach the shard as a `0` directory slot. */
    knownEmpty: string[];
    /** Summed `bytes` of {@link TerrainResolution.cells}. */
    bytes: number;
    /** Ground the selection covers that the terrain store says nothing about at
     *  all — outside the published coverage. Legal (the shard's directory says
     *  `0` there too) but not the same thing as canonically void, and shown as
     *  what it is: elevation this map will not have. */
    missing: string[];
}

/** A selection covering no raster at all — the answer for a terrain-less catalog. */
export function emptyTerrainResolution(): TerrainResolution {
    return { cells: [], knownEmpty: [], bytes: 0, missing: [] };
}

export interface SelectionResolution {
    parts: PartResolution[];
    /** The terrain squares this selection covers. Empty when the catalog has no
     *  terrain block or the index has not loaded. */
    terrain: TerrainResolution;
    /** The union, band id → sorted canonical cell ids that the catalog covers.
     *  Artifact entries contribute downloaded payloads; known-empty identities
     *  reach assembly without a payload. */
    cellsByBand: Map<string, string[]>;
    /** Ground the selection covers for which no cell is published, per band —
     *  the holes. Legal by construction (a missing cell is an empty leaf and the
     *  renderer paints backdrop there), which is exactly why they have to be
     *  shown rather than merely tolerated. */
    missingByBand: Map<string, string[]>;
    /** Bands named by the schema that have no loaded index. Empty in normal
     *  operation; non-empty means a price that is not yet the whole price. */
    unresolvedBands: string[];
    /** Ids of the parts whose contribution has not arrived yet ({@link
     *  PartResolution.pending}). The other half of "this price is not final":
     *  `unresolvedBands` is a column missing, this is a row. */
    unresolvedParts: string[];
}

/** A selection with nothing in it. */
export function emptySelection(corridorRadiusM: number): Selection {
    return { parts: [], corridorRadiusM };
}

/**
 * Add a part, or replace the one with the same id **in place**. Returns a new
 * selection — nothing here mutates, so a Svelte `$state` holding one re-renders
 * on assignment rather than on a deep proxy.
 *
 * In place matters: §8 U2's parts list is a ledger the user reads, and a row
 * that jumped to the bottom every time its box was nudged or its corridor
 * renamed would be a list that reorders itself while someone is looking at it.
 * A new part still goes on the end, where it was just added.
 */
export function withPart(selection: Selection, part: SelectionPart): Selection {
    const at = selection.parts.findIndex((p) => p.id === part.id);
    if (at < 0) return { ...selection, parts: [...selection.parts, part] };
    const parts = [...selection.parts];
    parts[at] = part;
    return { ...selection, parts };
}

/** Remove a part by id. */
export function withoutPart(selection: Selection, partId: string): Selection {
    return { ...selection, parts: selection.parts.filter((p) => p.id !== partId) };
}

/** Set the one global corridor width (§8 U3). */
export function withCorridorRadius(selection: Selection, radiusM: number): Selection {
    return { ...selection, corridorRadiusM: Math.max(0, radiusM) };
}

/**
 * §11.7's cross-document MUST, re-applied here over the full set of loaded
 * indices — which is the set the client could not check when it fetched the
 * region list one round trip earlier.
 *
 * It matters because without it a region cell the catalog does not index would
 * fall through the published/missing split below into `missingByBand`, and
 * `missingByBand` is drawn as a **hole**: ground with no published cell, legal
 * by construction, priced at nothing, shown to the rider as coverage they are
 * choosing to accept. But this is not that. A hole is ground nobody baked; this
 * is a *named* cell with no bytes, no size and no digest — a broken publish,
 * exactly what `assertRegionCellsIndexed` exists to refuse and what `client.ts`
 * promises callers they will never be handed. Two different failures with two
 * different remedies must not arrive as the same drawing.
 */
function assertRegionListsIndexed(selection: Selection, ctx: SelectionContext): void {
    const checked = new Set<string>();
    for (const part of selection.parts) {
        if (part.kind !== "region" || checked.has(part.regionId)) continue;
        checked.add(part.regionId);
        const list = ctx.regionCells.get(part.regionId);
        if (list) assertRegionCellsIndexed(list, ctx.indices);
    }
}

/**
 * The cell ids one part contributes to one band, before any published/missing
 * distinction — pure geometry (or, for a region, pure lookup).
 */
function partCells(part: SelectionPart, bandEntry: BandEntry, ctx: SelectionContext, radiusM: number): string[] {
    switch (part.kind) {
        case "region": {
            const list = ctx.regionCells.get(part.regionId);
            // A region whose list has not been fetched contributes nothing yet.
            // Not an error: the UI adds the part and the list arrives a moment
            // later, and a resolution that threw would make that a crash rather
            // than a frame.
            //
            // `hasOwn`, because band ids are document strings and
            // `"constructor"` is a legal one.
            if (!list || !Object.hasOwn(list.cells, bandEntry.id)) return [];
            return [...list.cells[bandEntry.id]];
        }
        case "box":
            return cellsIntersecting(bandEntry.cell_log2, part.box).map(formatCellId);
        case "corridor":
            return corridorCells(bandEntry.cell_log2, part.points, radiusM).map(formatCellId);
    }
}

/**
 * The terrain squares one part covers — the OBCA §1.2 coverage rule, verbatim,
 * applied to the terrain grid (§13.3 says a region's list is built the same way).
 *
 * There is no terrain special case here and there must never be one: a box and a
 * corridor resolve through exactly the test the bands use, and a region reads its
 * published list for the same reason it does for bands — deriving one from the
 * drawable boundary would let a simplification error drop an edge square, and a
 * dropped square is a stretch of route with no elevation at all.
 */
function partTerrainCells(part: SelectionPart, log2: number, ctx: SelectionContext, radiusM: number): string[] {
    switch (part.kind) {
        case "region":
            return [...(ctx.regionCells.get(part.regionId)?.terrain ?? [])];
        case "box":
            return cellsIntersecting(log2, part.box).map(formatCellId);
        case "corridor":
            return corridorCells(log2, part.points, radiusM).map(formatCellId);
    }
}

/** One part's contribution to one band, split — the unit the resolver caches. */
interface BandCells {
    published: string[];
    missing: string[];
}

/** {@link partCells}, split against the band's index and sorted. Canonical ids
 *  sort lexicographically into `(i, j)` order (the padding width is fixed per
 *  band, `OBCA_Spec.md` §1.3), so a plain string compare matches the order the
 *  catalog publishes. */
function classify(
    part: SelectionPart,
    bandEntry: BandEntry,
    ctx: SelectionContext,
    radiusM: number,
    index: CellIndexDocument,
): BandCells {
    const published: string[] = [];
    const missing: string[] = [];
    for (const id of new Set(partCells(part, bandEntry, ctx, radiusM))) {
        (cellIndexHas(index, id) ? published : missing).push(id);
    }
    return { published: published.sort(), missing: missing.sort() };
}

/** What a resolution needs from a (part, band) pair, however it is obtained. */
type CellSource = (part: SelectionPart, bandEntry: BandEntry, index: CellIndexDocument) => BandCells;

/**
 * Resolve a selection into the cells it names, per band, with per-part
 * attribution.
 *
 * This is the one-shot path: it computes every part from scratch. A UI holding
 * a slider does not want that sixty times a second — see {@link
 * SelectionResolver}.
 */
export function resolveSelection(selection: Selection, ctx: SelectionContext): SelectionResolution {
    assertRegionListsIndexed(selection, ctx);
    return resolveWith(selection, ctx, (part, bandEntry, index) =>
        classify(part, bandEntry, ctx, selection.corridorRadiusM, index),
    );
}

function resolveWith(selection: Selection, ctx: SelectionContext, cellsFor: CellSource): SelectionResolution {
    const bands = ctx.catalog.schema.bands;
    const unresolvedBands = bands.filter((b) => !ctx.indices.has(b.id)).map((b) => b.id);

    // Pass one: every part's raw cells per band, split into published and missing.
    const perPart = selection.parts.map((part) => {
        const cellsByBand = new Map<string, string[]>();
        const missingByBand = new Map<string, string[]>();
        for (const bandEntry of bands) {
            const index = ctx.indices.get(bandEntry.id);
            if (!index) continue;
            const { published, missing } = cellsFor(part, bandEntry, index);
            // Copies: the lists may be a cache's, and a resolution is handed to
            // a UI that is entitled to sort or splice what it was given.
            if (published.length) cellsByBand.set(bandEntry.id, [...published]);
            if (missing.length) missingByBand.set(bandEntry.id, [...missing]);
        }
        // A region whose list has not arrived contributes nothing *yet*, which
        // on its own is indistinguishable from contributing nothing.
        const pending = part.kind === "region" && !ctx.regionCells.has(part.regionId);
        return { part, cellsByBand, missingByBand, pending };
    });

    // Pass two: the union, and how many parts contribute each cell — which is
    // what makes "what does removing this save?" answerable.
    const union = new Map<string, Set<string>>();
    const missingUnion = new Map<string, Set<string>>();
    const contributors = new Map<string, number>();
    const key = (band: string, id: string) => `${band} ${id}`;
    for (const { cellsByBand, missingByBand } of perPart) {
        for (const [band, ids] of cellsByBand) {
            const into = union.get(band) ?? new Set<string>();
            for (const id of ids) {
                into.add(id);
                contributors.set(key(band, id), (contributors.get(key(band, id)) ?? 0) + 1);
            }
            union.set(band, into);
        }
        for (const [band, ids] of missingByBand) {
            const into = missingUnion.get(band) ?? new Set<string>();
            for (const id of ids) into.add(id);
            missingUnion.set(band, into);
        }
    }

    const bytesOf = (band: string, id: string) => ctx.indices.get(band)?.byId.get(id)?.bytes ?? 0;

    // The raster: one union over every part, split three ways against the pinned
    // terrain index. Not per part, because §13.3 prices terrain as one number for
    // the selection and the parts list has no terrain column — the ledger shows
    // one "elevation" line, which is what the "no toggle" decision implies.
    const terrainIndex = ctx.terrain ?? null;
    const terrain = emptyTerrainResolution();
    if (terrainIndex) {
        const seen = new Set<string>();
        for (const part of selection.parts) {
            for (const id of partTerrainCells(part, terrainIndex.cell_log2, ctx, selection.corridorRadiusM)) {
                seen.add(id);
            }
        }
        for (const id of [...seen].sort()) {
            const entry = terrainIndex.byId.get(id);
            if (entry) {
                terrain.cells.push(id);
                terrain.bytes += entry.bytes;
            } else if (terrainEmptyAt(terrainIndex, id)) {
                terrain.knownEmpty.push(id);
            } else {
                terrain.missing.push(id);
            }
        }
    }

    const parts: PartResolution[] = perPart.map(({ part, cellsByBand, missingByBand, pending }) => {
        let bytes = 0;
        let marginalBytes = 0;
        let cellCount = 0;
        for (const [band, ids] of cellsByBand) {
            cellCount += ids.length;
            for (const id of ids) {
                const size = bytesOf(band, id);
                bytes += size;
                if ((contributors.get(key(band, id)) ?? 0) === 1) marginalBytes += size;
            }
        }
        let missingCount = 0;
        for (const ids of missingByBand.values()) missingCount += ids.length;
        return { part, cellsByBand, cellCount, bytes, marginalBytes, missingByBand, missingCount, pending };
    });

    return {
        parts,
        terrain,
        cellsByBand: sortedLists(union),
        missingByBand: sortedLists(missingUnion),
        unresolvedBands,
        unresolvedParts: parts.filter((p) => p.pending).map((p) => p.part.id),
    };
}

function sortedLists(source: Map<string, Set<string>>): Map<string, string[]> {
    const out = new Map<string, string[]>();
    for (const [band, ids] of source) out.set(band, [...ids].sort());
    return out;
}

/** One cached answer, with everything it depended on, so staleness is a
 *  comparison rather than a convention. */
interface CachedBand {
    part: SelectionPart;
    radiusM: number;
    index: CellIndexDocument;
    regionList: RegionCellsDocument | undefined;
    cells: BandCells;
}

/**
 * `resolveSelection`, but remembering what it worked out.
 *
 * The reason this exists is the corridor slider. Resolving a bikepacking-sized
 * selection is tens of milliseconds — the geometry is per band, per part, per
 * segment — and a slider asks for a new answer on every frame it moves. Almost
 * none of that work is new: a global corridor width (§8 U3) changes what the
 * *corridor* parts cover and nothing else, so a map of two regions and a drawn
 * box recomputes three parts to answer a question about none of them.
 *
 * So the cache is keyed per **(part, band)** — the finest grain at which an
 * answer is reusable — and an entry is reused only when everything it was
 * computed from is still the same object: the part itself, the band's index
 * document, the region's cell list, and (for corridors) the radius. Parts are
 * values here, replaced rather than edited by `withPart`, so identity is the
 * right test and a caller that does edit one in place can say so with
 * {@link invalidate}.
 *
 * Deliberately UI-free: no store, no `$state`, no framework. It is a data
 * structure with a lifetime, and the component that owns one is the component
 * that decides when it dies.
 */
export class SelectionResolver {
    private readonly cache = new Map<string, CachedBand>();
    /** The last index set a region's list was checked against, so §11.7's
     *  cross-document check is not re-walked on every frame either. */
    private readonly checked = new WeakMap<RegionCellsDocument, ReadonlyMap<string, CellIndexDocument>>();
    /** Cache hits and misses, for tests and for anyone wondering where a frame
     *  went. Not load-bearing. */
    readonly stats = { computed: 0, reused: 0 };

    /** Drop everything remembered about one part — the escape hatch for a
     *  caller that mutates a part in place instead of replacing it. */
    invalidate(partId: string): void {
        for (const key of [...this.cache.keys()]) {
            if (key.slice(0, key.indexOf(" ")) === partId) this.cache.delete(key);
        }
    }

    /** Drop everything. */
    invalidateAll(): void {
        this.cache.clear();
    }

    /** How many (part, band) answers are currently remembered. */
    get size(): number {
        return this.cache.size;
    }

    resolve(selection: Selection, ctx: SelectionContext): SelectionResolution {
        this.assertRegionLists(selection, ctx);
        const live = new Set<string>();
        const resolution = resolveWith(selection, ctx, (part, bandEntry, index) => {
            const key = `${part.id} ${bandEntry.id}`;
            live.add(key);
            const regionList = part.kind === "region" ? ctx.regionCells.get(part.regionId) : undefined;
            const hit = this.cache.get(key);
            if (
                hit &&
                hit.part === part &&
                hit.index === index &&
                hit.regionList === regionList &&
                (part.kind !== "corridor" || hit.radiusM === selection.corridorRadiusM)
            ) {
                this.stats.reused += 1;
                return hit.cells;
            }
            this.stats.computed += 1;
            const cells = classify(part, bandEntry, ctx, selection.corridorRadiusM, index);
            this.cache.set(key, { part, radiusM: selection.corridorRadiusM, index, regionList, cells });
            return cells;
        });
        // A removed part must not keep its answers alive: a session that adds
        // and removes fifty boxes should not be holding fifty answers.
        for (const key of [...this.cache.keys()]) {
            if (!live.has(key)) this.cache.delete(key);
        }
        return resolution;
    }

    private assertRegionLists(selection: Selection, ctx: SelectionContext): void {
        for (const part of selection.parts) {
            if (part.kind !== "region") continue;
            const list = ctx.regionCells.get(part.regionId);
            if (!list || this.checked.get(list) === ctx.indices) continue;
            assertRegionCellsIndexed(list, ctx.indices);
            this.checked.set(list, ctx.indices);
        }
    }
}
