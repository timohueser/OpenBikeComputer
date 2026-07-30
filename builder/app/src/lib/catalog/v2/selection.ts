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
import type { BandEntry, CatalogV2 } from "./manifest";
import type { CellIndexDocument, RegionCellsDocument } from "./satellites";

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
    catalog: CatalogV2;
    /** Band id → its cell index. A band with no index resolves to nothing and
     *  says so; it does not silently drop out of the map. */
    indices: ReadonlyMap<string, CellIndexDocument>;
    /** Region id → its published cell list, for every `region` part in play. */
    regionCells: ReadonlyMap<string, RegionCellsDocument>;
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
}

export interface SelectionResolution {
    parts: PartResolution[];
    /** The union, band id → sorted canonical cell ids that the catalog
     *  publishes. This is what gets downloaded and assembled. */
    cellsByBand: Map<string, string[]>;
    /** Ground the selection covers for which no cell is published, per band —
     *  the holes. Legal by construction (a missing cell is an empty leaf and the
     *  renderer paints backdrop there), which is exactly why they have to be
     *  shown rather than merely tolerated. */
    missingByBand: Map<string, string[]>;
    /** Bands named by the schema that have no loaded index. Empty in normal
     *  operation; non-empty means a price that is not yet the whole price. */
    unresolvedBands: string[];
}

/** A selection with nothing in it. */
export function emptySelection(corridorRadiusM: number): Selection {
    return { parts: [], corridorRadiusM };
}

/** Add a part, or replace the one with the same id. Returns a new selection —
 *  nothing here mutates, so a Svelte `$state` holding one re-renders on
 *  assignment rather than on a deep proxy. */
export function withPart(selection: Selection, part: SelectionPart): Selection {
    const parts = selection.parts.filter((p) => p.id !== part.id);
    return { ...selection, parts: [...parts, part] };
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
            return list ? [...(list.cells[bandEntry.id] ?? [])] : [];
        }
        case "box":
            return cellsIntersecting(bandEntry.cell_log2, part.box).map(formatCellId);
        case "corridor":
            return corridorCells(bandEntry.cell_log2, part.points, radiusM).map(formatCellId);
    }
}

/**
 * Resolve a selection into the cells it names, per band, with per-part
 * attribution.
 *
 * Canonical ids sort lexicographically into `(i, j)` order — the padding width
 * is fixed per band (`OBCA_Spec.md` §1.3) — so every list here is sorted with a
 * plain string compare and matches the order the catalog publishes.
 */
export function resolveSelection(selection: Selection, ctx: SelectionContext): SelectionResolution {
    const bands = ctx.catalog.schema.bands;
    const unresolvedBands = bands.filter((b) => !ctx.indices.has(b.id)).map((b) => b.id);

    // Pass one: every part's raw cells per band, split into published and missing.
    const perPart = selection.parts.map((part) => {
        const cellsByBand = new Map<string, string[]>();
        const missingByBand = new Map<string, string[]>();
        for (const bandEntry of bands) {
            const index = ctx.indices.get(bandEntry.id);
            if (!index) continue;
            const published: string[] = [];
            const missing: string[] = [];
            for (const id of new Set(partCells(part, bandEntry, ctx, selection.corridorRadiusM))) {
                (index.byId.has(id) ? published : missing).push(id);
            }
            if (published.length) cellsByBand.set(bandEntry.id, published.sort());
            if (missing.length) missingByBand.set(bandEntry.id, missing.sort());
        }
        return { part, cellsByBand, missingByBand };
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

    const parts: PartResolution[] = perPart.map(({ part, cellsByBand, missingByBand }) => {
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
        return { part, cellsByBand, cellCount, bytes, marginalBytes, missingByBand, missingCount };
    });

    return {
        parts,
        cellsByBand: sortedLists(union),
        missingByBand: sortedLists(missingUnion),
        unresolvedBands,
    };
}

function sortedLists(source: Map<string, Set<string>>): Map<string, string[]> {
    const out = new Map<string, string[]>();
    for (const [band, ids] of source) out.set(band, [...ids].sort());
    return out;
}
