// Exact selection pricing and coverage warnings. Totals sum published cell
// bytes. Missing cells are holes, while partial coarse cells are tracked as
// normal context rather than warning-hatched detail.
//
// Nothing here judges a size. A map is one object whose interior scales to
// 64 GiB (`OBCM_Spec.md` §1.1), so the only size question left is whether it
// fits the rider's card — which the device answers when the bytes arrive
// (`lib/device/write.ts`), not the catalog beforehand.

import type { BandRole, Catalog, RegionEntry } from "./manifest";
import type { CellIndexDocument } from "./satellites";
import type { SelectionResolution } from "./selection";

/** One band's line of the ledger. */
export interface BandLedger {
    band: string;
    role: BandRole;
    cellCount: number;
    /** Summed real cell bytes — what the download costs for this band. */
    bytes: number;
    /** True for the coarse band: context around the selection rather than
     *  content in it, and silent in the UI by §8's coarse-band decision. */
    contextOnly: boolean;
    /** Published cells in this band whose sources do not cover their whole
     *  square. */
    partialCells: string[];
    /** Ground in this band with no published cell at all. */
    missingCells: string[];
}

/** The coverage story, split so a UI can hatch exactly what deserves hatching. */
export interface CoverageReport {
    /** Ground the selection covers with no published cell, per band. */
    holesByBand: Map<string, string[]>;
    holeCount: number;
    /** Partial cells in bands a rider reads detail from — the ones worth a
     *  warning inside the selection. */
    partialDetailByBand: Map<string, string[]>;
    partialDetailCount: number;
    /** Partial cells in the coarse context band. Normal at country scale, kept
     *  separate so nothing hatches a whole map over them. */
    partialContextCount: number;
    /** Whether there is anything to draw a warning for at all — holes anywhere,
     *  or partial cells in a detail band. Never true for context alone. */
    hasWarnings: boolean;
}

/**
 * The elevation line (EL4, `OBCC_Spec.md` §13.3).
 *
 * Kept beside the bands rather than as one of them, because terrain is a second
 * artifact class with its own revision track — it is not in `bytes_by_band`.
 * What it does share is §5.7's discipline: every byte here is a published
 * `bytes` the catalog states, summed before anything is fetched.
 */
export interface TerrainLedger {
    /** Downloadable squares. */
    cellCount: number;
    /** Squares that are canonically void — coverage that costs nothing (§13.6). */
    knownEmptyCount: number;
    /** Ground with no terrain object *and* no void assertion: elevation this map
     *  will not have. Legal, and shown rather than merely tolerated. */
    missingCount: number;
    /** Summed published `bytes` — what the raster adds to the download. */
    bytes: number;
    /** The catalog's source credit, verbatim (§13.5). A consumer that displays
     *  terrain MUST show this and MUST NOT hard-code it. */
    attribution: string;
}

export interface Ledger {
    bands: BandLedger[];
    /** Everything the download costs: the sum of every selected cell's bytes,
     *  across every band **and the raster**. This is the number the summary card
     *  shows, because it is what the transfer actually costs. */
    totalBytes: number;
    cellCount: number;
    /** The raster's own line, or `null` when the catalog publishes no terrain —
     *  a complete map whose profiles are flat (§13). */
    terrain: TerrainLedger | null;
    /** The core band's line — the nav graph and the POIs. The disk-need
     *  projection prices it apart from the geometry (`DownloadStep`). */
    core: BandLedger;
    coverage: CoverageReport;
    /** Bands with no loaded index: their bytes are missing from the total, so a
     *  UI must not present it as final. Empty in normal operation. */
    unresolvedBands: string[];
    /** Parts whose cell list has not arrived: their bytes are missing from the
     *  total too, and for the same reason it must not be presented as final.
     *  Ids of `region` parts, mid-fetch. */
    unresolvedParts: string[];
    /**
     * Whether every band and every part has reported in.
     *
     * The one thing a summary card must consult before it prints a total. A
     * pending region contributes 0 B, which is a perfectly ordinary number, so
     * "DACH — 0 B, no holes" is what a confident card says half a second before
     * it says 47 GB. Nothing about the total itself can tell the two apart.
     */
    isFinal: boolean;
}

function coverageReport(
    holesByBand: Map<string, string[]>,
    bands: BandLedger[],
    partialCountByBand?: Record<string, number>,
): CoverageReport {
    let holeCount = 0;
    for (const ids of holesByBand.values()) holeCount += ids.length;
    const partialDetailByBand = new Map<string, string[]>();
    let partialDetailCount = 0;
    let partialContextCount = 0;
    for (const band of bands) {
        const partialCount = partialCountByBand && Object.hasOwn(partialCountByBand, band.band)
            ? partialCountByBand[band.band]
            : band.partialCells.length;
        if (!partialCount) continue;
        if (band.contextOnly) {
            partialContextCount += partialCount;
            continue;
        }
        // A root-only price knows the count but not the cell ids. Keep the id
        // map honest (nothing can be hatched yet) while still applying the
        // coarse-context rule to the summary/warning count.
        if (band.partialCells.length) partialDetailByBand.set(band.band, band.partialCells);
        partialDetailCount += partialCount;
    }
    return {
        holesByBand,
        holeCount,
        partialDetailByBand,
        partialDetailCount,
        partialContextCount,
        hasWarnings: holeCount > 0 || partialDetailCount > 0,
    };
}

/**
 * Price a resolved selection.
 *
 * Every byte here came from a `CellEntry.bytes` the bakery published; nothing is
 * derived from area, density or the drawable boundary.
 */
export function ledgerFor(
    resolution: SelectionResolution,
    catalog: Catalog,
    indices: ReadonlyMap<string, CellIndexDocument>,
): Ledger {
    const bands: BandLedger[] = catalog.schema.bands.map((band) => {
        const index = indices.get(band.id);
        const ids = resolution.cellsByBand.get(band.id) ?? [];
        let bytes = 0;
        const partialCells: string[] = [];
        for (const id of ids) {
            const cell = index?.byId.get(id);
            if (!cell) continue;
            bytes += cell.bytes;
            if (cell.partial) partialCells.push(id);
        }
        return {
            band: band.id,
            role: band.role,
            cellCount: ids.length,
            bytes,
            contextOnly: band.role === "coarse",
            partialCells,
            missingCells: resolution.missingByBand.get(band.id) ?? [],
        };
    });

    const core = bands.find((b) => b.role === "core")!;
    const terrain: TerrainLedger | null = catalog.terrain
        ? {
              cellCount: resolution.terrain.cells.length,
              knownEmptyCount: resolution.terrain.knownEmpty.length,
              missingCount: resolution.terrain.missing.length,
              bytes: resolution.terrain.bytes,
              attribution: catalog.terrain.attribution,
          }
        : null;
    return {
        bands,
        terrain,
        totalBytes: bands.reduce((sum, b) => sum + b.bytes, 0) + (terrain?.bytes ?? 0),
        cellCount: bands.reduce((sum, b) => sum + b.cellCount, 0),
        core,
        coverage: coverageReport(resolution.missingByBand, bands),
        unresolvedBands: resolution.unresolvedBands,
        unresolvedParts: resolution.unresolvedParts,
        isFinal: resolution.unresolvedBands.length === 0 && resolution.unresolvedParts.length === 0,
    };
}

/**
 * Price a named region straight from the root document — no satellite fetch.
 *
 * This is `OBCC_Spec.md` §6's whole reason for putting `bytes`,
 * `bytes_by_band` and `cell_count` in the root: a builder must be able to price
 * a region the moment a rider hovers it, and pricing must not cost a round trip.
 * The result is the same shape as {@link ledgerFor}.
 *
 * Per-band partial counts apply the same coarse-context rule before the
 * satellite fetch.
 */
export function ledgerForRegion(catalog: Catalog, entry: RegionEntry): Ledger {
    // `hasOwn`, not `?? 0`: a band id is a document string and `"constructor"`
    // is a legal one, so a plain lookup can answer with an inherited function
    // that `??` happily passes through into the arithmetic.
    const numberAt = (map: Record<string, number>, key: string) => (Object.hasOwn(map, key) ? map[key] : 0);
    const bands: BandLedger[] = catalog.schema.bands.map((band) => {
        const bytes = numberAt(entry.bytes_by_band, band.id);
        return {
            band: band.id,
            role: band.role,
            cellCount: numberAt(entry.cell_count, band.id),
            bytes,
            contextOnly: band.role === "coarse",
            partialCells: [],
            missingCells: [],
        };
    });
    const core = bands.find((b) => b.role === "core")!;
    // §13.3 prices a region's raster in the root too, so hovering a region shows
    // the whole download — map plus elevation — with no satellite fetch either.
    const terrain: TerrainLedger | null =
        catalog.terrain && entry.terrain
            ? {
                  cellCount: entry.terrain.cell_count,
                  knownEmptyCount: entry.terrain.known_empty_count,
                  missingCount: 0,
                  bytes: entry.terrain.bytes,
                  attribution: catalog.terrain.attribution,
              }
            : null;
    return {
        bands,
        terrain,
        totalBytes: entry.bytes + (terrain?.bytes ?? 0),
        cellCount: bands.reduce((sum, b) => sum + b.cellCount, 0),
        core,
        coverage: coverageReport(new Map(), bands, entry.partial_cell_count_by_band),
        unresolvedBands: [],
        unresolvedParts: [],
        // The root prices a region completely — that is §6's whole point —
        // so this answer is final the moment the catalog is loaded.
        isFinal: true,
    };
}
