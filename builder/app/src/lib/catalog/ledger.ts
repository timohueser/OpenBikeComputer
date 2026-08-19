// Exact selection pricing and coverage warnings. Totals sum published cell
// bytes; only the unsplittable core band is judged against the per-file ceiling.
// Missing cells are holes, while partial coarse cells are tracked as normal
// context rather than warning-hatched detail.

import type { BandRole, Catalog, RegionEntry } from "./manifest";
import type { CellIndexDocument } from "./satellites";
import type { SelectionResolution } from "./selection";

/**
 * The per-file ceiling this consumer refuses above — deliberately **not** the engine's.
 *
 * It was justified by two limits that landed on one number: FAT32's largest file, and the largest
 * offset an OBCM `uint32` could name. Both are retired (OBCM v14's scaled offsets, and the flat
 * store), and the assembler's own wall is now `emit::FILE_CEILING` at 64 GiB.
 *
 * **The gate stays at `4 GiB − 1` because the thing a rider actually puts a map on has not been cut
 * over yet.** The device still mounts a FAT32 card, where a file cannot exceed this whatever the
 * format can express — so for every device in existence today it is a live limit, not a historical
 * one, and a builder that let a rider download 6 GiB it could not then write would be lying about
 * the only number that matters to them. It lifts with the board cutover (FS7.5c), which is what
 * replaces the card's filesystem; until then this is the honest ceiling for a *consumer* even
 * though it is no longer the producer's.
 *
 * (A second thing waits for the same slice: this ledger judges the **core band** against the
 * ceiling, which was the volume set's model — the core could not split and the geometry could. One
 * file has no such split, so what should be judged is the whole assembly. Changing that means
 * changing what the projection is *of*, and it is only meaningful once the card can hold more than
 * this constant anyway.)
 */
export const MAX_FILE_BYTES = 4 * 1024 ** 3 - 1;

/** Where the warning starts: seven eighths of {@link MAX_FILE_BYTES}, the proportion §5.7 has always
 *  used for "you are close". */
export const CORE_WARN_BYTES = 3.5 * 1024 ** 3;

/**
 * The pessimistic budget §5.7 requires on the comparison side: +15 %, from
 * `OBCA_Spec.md` §1.5's per-cell overhead allowance.
 *
 * §1.5 is explicit that this is **measured headroom rather than an expected
 * cost** — a real scoped bake came in 0–4 % *smaller* than the whole-extract
 * figures — and equally explicit that it stays, because §5.7 requires the
 * pre-download projection to be an upper bound and a budget that is never
 * exceeded is doing its job. It is the right headroom for this consumer too:
 * what a projection has to bound is the *assembled* file, and assembly adds
 * what the cells do not carry — fresh upper index nodes, offset tables,
 * directories, and the merged POI/hours and nav sections (§5.7's "fixed
 * overheads"). Sizing those exactly is the assembler's job (P3); refusing a
 * selection that would need them to be tiny is this one's.
 */
export const OVERHEAD_BUDGET = 0.15;

/** One band's line of the ledger. */
export interface BandLedger {
    band: string;
    role: BandRole;
    cellCount: number;
    /** Summed real cell bytes — what the download costs for this band. */
    bytes: number;
    /** The same, carrying §5.7's pessimistic budget: what the *assembled* file
     *  is projected to come to, and therefore the number every ceiling is
     *  compared against. Never the number to show as "the download size". */
    projectedBytes: number;
    /** False only for the core, the one file of a set that cannot be split by
     *  bbox — which is why it is the only one with a ceiling it can hit. */
    splittable: boolean;
    /** True for the coarse band: context around the selection rather than
     *  content in it, and silent in the UI by §8's coarse-band decision. */
    contextOnly: boolean;
    /** Published cells in this band whose sources do not cover their whole
     *  square. */
    partialCells: string[];
    /** Ground in this band with no published cell at all. */
    missingCells: string[];
}
/**
 * What was compared with what, and the sentence that says so.
 *
 * Both numbers, deliberately. §5.7 judges the **projected** size — the cells
 * plus §1.5's budget for what assembly adds — and that is the only number whose
 * relation to `limit` is meaningful: `projectedBytes > limit` holds in every
 * `warn` and every `refuse`, so a meter drawn from this payload sits past the
 * line exactly when the verdict says it does. `nominalBytes` is the summed cell
 * bytes, the honest answer to "how big is the download", and it is here so a UI
 * can show what it costs beside why it was refused rather than choosing one and
 * contradicting the other. Quoting only the nominal figure is how a refusal
 * ends up saying "about 3.6 GiB, past the 4 GiB limit".
 */
export interface LedgerJudgement {
    band: string;
    /** Summed real cell bytes for the core band. */
    nominalBytes: number;
    /** The same carrying §5.7's budget — the figure compared with `limit`. */
    projectedBytes: number;
    limit: number;
    message: string;
}

export type LedgerVerdict =
    | { kind: "ok" }
    | ({ kind: "warn" } & LedgerJudgement)
    | ({ kind: "refuse" } & LedgerJudgement);

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
 * artifact class with its own revision track — it is not in `bytes_by_band`, it
 * has no per-file ceiling to test (it is always its own file, `OBCA_Spec.md`
 * §5.5), and it never feeds the core's nav-graph verdict. What it does share is
 * §5.7's discipline: every byte here is a published `bytes` the catalog states,
 * summed before anything is fetched.
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
    /** The core file's line — the nav graph and the POIs. */
    core: BandLedger;
    coverage: CoverageReport;
    verdict: LedgerVerdict;
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
 * §5.7's judgement, in the words §5.7 requires: the refusal and the warning both
 * name the **navigation graph** as the reason and the coverage as the thing to
 * reduce. That is not decoration — after §5.1 put geometry in splittable shards,
 * the core is nav plus POIs and nothing else, so no other explanation would be
 * true, and "your map is too big" would send a rider to the wrong fix.
 */
function judge(core: BandLedger): LedgerVerdict {
    const judged: Omit<LedgerJudgement, "limit" | "message"> = {
        band: core.band,
        nominalBytes: core.bytes,
        projectedBytes: core.projectedBytes,
    };
    // Both figures in the sentence, in the order a rider reads them: what the
    // download is, and what it becomes. The second is the one being judged, and
    // a sentence that named only the first would spend the whole refuse band
    // (3.48–4.0 GiB of cells) quoting a number below the limit it is citing.
    const cells = `about ${gib(core.bytes)} GiB of cells, about ${gib(core.projectedBytes)} GiB once assembled`;
    if (core.projectedBytes > MAX_FILE_BYTES) {
        return {
            kind: "refuse",
            ...judged,
            limit: MAX_FILE_BYTES,
            message:
                `This selection's navigation graph alone comes to ${cells} — past the 4 GiB a file on the ` +
                "device's card can hold. Reduce the coverage — fewer regions, a narrower corridor — and the " +
                "rest of the map will follow.",
        };
    }
    if (core.projectedBytes > CORE_WARN_BYTES) {
        return {
            kind: "warn",
            ...judged,
            limit: CORE_WARN_BYTES,
            message:
                `This selection's navigation graph is ${cells}, close to the 4 GiB a file on the device's ` +
                "card can hold. A little less coverage would leave more room.",
        };
    }
    return { kind: "ok" };
}

/** GiB to two decimals — the spelling the messages and the tests share, so the
 *  sentence and the payload cannot drift apart. */
export function gib(bytes: number): string {
    return (bytes / 1024 ** 3).toFixed(2);
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
            projectedBytes: Math.ceil(bytes * (1 + OVERHEAD_BUDGET)),
            splittable: band.role !== "core",
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
        verdict: judge(core),
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
 * The result is the same shape as {@link ledgerFor} and the same verdict
 * arithmetic runs on it.
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
            projectedBytes: Math.ceil(bytes * (1 + OVERHEAD_BUDGET)),
            splittable: band.role !== "core",
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
        verdict: judge(core),
        unresolvedBands: [],
        unresolvedParts: [],
        // The root prices a region completely — that is §6's whole point —
        // so this answer is final the moment the catalog is loaded.
        isFinal: true,
    };
}
