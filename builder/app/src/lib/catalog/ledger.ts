// The size ledger: what a selection costs, which file the cost lands in, and
// whether it can legally be built at all.
//
// This is epic #1016 §8 U4 — the always-visible "Map summary" card — as
// arithmetic. Three rules shape every line of it.
//
// **1. Totals are summed real cell bytes. Never an estimate.** PR #1025 measured
// the trap: a selection's cell squares cover 1.5–1.8× the ground its region
// does, because border cells carry the neighbour's side as well. Priced by area
// × density that would overstate Freiburg by 50 %; priced by summing the cells'
// own published `bytes` it is exact, because the over-covered ground is outside
// the extract and carries no bytes. `OBCC_Spec.md` §6 publishes `bytes` per
// cell and `bytes_by_band` per region precisely so a consumer never has to
// estimate, and this module never does.
//
// **2. The ceiling is a per-file ceiling, and only one file has it.** A map is a
// volume set (`OBCA_Spec.md` §5.1): the **core** file carries the nav graph and
// the POIs and cannot be split by bbox, while coarse and geometry shards split
// as needed. §5.7 makes the projection mandatory *before* the download and the
// refusal absolute — and after the split the only file that can approach the
// ceiling without a remedy is the core. So the core band's bytes are judged
// against `4 GiB − 1` (refuse) and ≈ 3.5 GiB (warn), and both sentences name the
// navigation graph as the reason, because after §5.1 no other explanation is
// true.
//
// **3. A partial coarse cell is not a hole, and at country scale it is not even
// news.** #1025 again: a `2^20` cell is ≈ 9 100 km², so *no* coarse cell is fully
// interior to Switzerland — all sixteen are `partial`. Hatching those under §8
// U1's "loud warnings inside the selection" would hatch the entire map for a
// single-country catalog. So this ledger separates three things that a naive
// count would merge: **holes** (ground with no published cell — real, any band),
// **partial detail cells** (a published cell whose sources do not cover its whole
// square, in a band a rider reads detail from — worth warning about), and
// **partial context cells** (the same thing in the coarse band, which is context
// rather than content — counted, exposed, and not a warning).

import type { BandRole, Catalog, RegionEntry } from "./manifest";
import type { CellIndexDocument } from "./satellites";
import type { SelectionResolution } from "./selection";

/** The hard per-file ceiling: FAT32's largest file, and also the largest offset
 *  an OBCM `uint32` can name. Two independent limits at the same number
 *  (`OBCA_Spec.md` §5.7). */
export const MAX_FILE_BYTES = 4 * 1024 ** 3 - 1;

/** Where the core file's warning starts (§5.7's "SHOULD warn above ≈ 3.5 GiB"). */
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
    /** The root's legacy unsplit count, set only when an older additive-v2
     *  catalog omits `partial_cell_count_by_band`. It is reported but cannot be
     *  classified as context or detail, so it never becomes a warning. */
    unsplitPartialCount: number | null;
    /** Whether there is anything to draw a warning for at all — holes anywhere,
     *  or partial cells in a detail band. Never true for context alone. */
    hasWarnings: boolean;
}

export interface Ledger {
    bands: BandLedger[];
    /** Everything the download costs: the sum of every selected cell's bytes,
     *  across every band. This is the number the summary card shows. */
    totalBytes: number;
    cellCount: number;
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
        unsplitPartialCount: null,
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
                `This selection's navigation graph alone comes to ${cells} — past the 4 GiB a single map file ` +
                "can hold. Reduce the coverage — fewer regions, a narrower corridor — and the rest of the map " +
                "will follow.",
        };
    }
    if (core.projectedBytes > CORE_WARN_BYTES) {
        return {
            kind: "warn",
            ...judged,
            limit: CORE_WARN_BYTES,
            message:
                `This selection's navigation graph is ${cells}, close to the 4 GiB one map file can hold. ` +
                "A little less coverage would leave more room.",
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
    return {
        bands,
        totalBytes: bands.reduce((sum, b) => sum + b.bytes, 0),
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
 * Current roots split partial counts by band, so the same coarse-context rule
 * applies before the satellite fetch. An older additive-v2 root may omit that
 * split; its total is preserved as `coverage.unsplitPartialCount` and cannot be
 * classified as a warning until the cell lists load.
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
    const splitPartials = entry.partial_cell_count_by_band;
    return {
        bands,
        totalBytes: entry.bytes,
        cellCount: bands.reduce((sum, b) => sum + b.cellCount, 0),
        core,
        coverage: {
            ...coverageReport(new Map(), bands, splitPartials ?? undefined),
            unsplitPartialCount: splitPartials ? null : entry.partial_cell_count,
        },
        verdict: judge(core),
        unresolvedBands: [],
        unresolvedParts: [],
        // The root prices a region completely — that is §6's whole point —
        // so this answer is final the moment the catalog is loaded.
        isFinal: true,
    };
}

/**
 * Does the map fit the card?
 *
 * §9/D4 replaced the old user-visible 4 GiB wall with this: a map is a volume
 * set of any number of files, so the only limit a rider ever sees is free space
 * on the SD card. The per-file ceiling still exists and is still absolute — it
 * is just the assembler's business and {@link Ledger.verdict}'s, not a number
 * anybody is shown.
 */
export function fitsOnCard(ledger: Ledger, freeBytes: number): { fits: boolean; shortfallBytes: number } {
    const need = Math.ceil(ledger.totalBytes * (1 + OVERHEAD_BUDGET));
    return { fits: need <= freeBytes, shortfallBytes: Math.max(0, need - freeBytes) };
}
