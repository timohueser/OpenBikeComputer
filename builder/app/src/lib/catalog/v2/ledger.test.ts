// The size ledger: real bytes, the core's ceiling, and what deserves a warning.
//
// Three findings are pinned here because losing any of them costs a user
// something concrete: a price that overstates a selection by half (PR #1025's
// area-times-density trap), a map that cannot legally be written (OBCA §5.7's
// core ceiling), and a coverage warning that hatches an entire country because
// its coarse cells are — normally, unavoidably — partial.

import { describe, expect, it } from "vitest";
import { fitsOnCard, gib, ledgerFor, ledgerForRegion, MAX_FILE_BYTES, OVERHEAD_BUDGET } from "./ledger";
import type { RegionEntry } from "./manifest";
import { resolveSelection, type BoxPart, type RegionPart, type SelectionContext } from "./selection";
import { cellSquare, parseCellId } from "./grid";
import { exampleCatalog, fixtureIndices } from "./testdata";

const GIB = 1024 ** 3;

const indices = fixtureIndices(exampleCatalog, {
    // Partial coarse cells are the normal state at country scale (#1025): a
    // 2^20 cell is ≈ 9 100 km², and no such cell is fully interior to
    // Switzerland.
    coarse: [{ id: "20/0301/0263", bytes: 2088, partial: true }],
    mid: [{ id: "19/0602/0526", bytes: 1064 }],
    fine: [
        { id: "18/1204/1052", bytes: 552 },
        { id: "18/1204/1053", bytes: 424, partial: true },
    ],
    network: [
        { id: "18/1204/1052", bytes: 296 },
        { id: "18/1204/1053", bytes: 168 },
    ],
});

const ctx: SelectionContext = { catalog: exampleCatalog, indices, regionCells: new Map() };
const A = cellSquare(parseCellId("18/1204/1052"));
const B = cellSquare(parseCellId("18/1204/1053"));

function box(id: string, span: { minLat: number; minLon: number; maxLat: number; maxLon: number }): BoxPart {
    return { kind: "box", id, name: id, box: span };
}

const overA = box("a", { minLat: A.minLat + 1, minLon: A.minLon + 1, maxLat: A.maxLat - 1, maxLon: A.maxLon - 1 });
const overAB = box("ab", { minLat: A.minLat + 1, minLon: A.minLon + 1, maxLat: A.maxLat - 1, maxLon: B.maxLon - 1 });

function ledgerOf(...parts: BoxPart[]) {
    return ledgerFor(resolveSelection({ parts, corridorRadiusM: 0 }, ctx), exampleCatalog, indices);
}

describe("ledgerFor", () => {
    it("totals summed real cell bytes, never an estimate", () => {
        // #1025: border cells carry the neighbour's overhang, so cell squares
        // cover 1.5–1.8× a region's ground. Anything derived from area × density
        // would be wrong by that much; these are the catalog's own numbers.
        const ledger = ledgerOf(overAB);
        expect(ledger.totalBytes).toBe(2088 + 1064 + 552 + 424 + 296 + 168);
        expect(ledger.cellCount).toBe(6);
        // The two fine cells differ by a third — an average-times-count price
        // would land somewhere else entirely.
        expect(ledger.bands.find((b) => b.band === "fine")!.bytes).toBe(552 + 424);
    });

    it("splits the total by band, and names the core file's line", () => {
        const ledger = ledgerOf(overA);
        expect(Object.fromEntries(ledger.bands.map((b) => [b.band, b.bytes]))).toEqual({
            coarse: 2088,
            mid: 1064,
            fine: 552,
            network: 296,
        });
        expect(ledger.core.band).toBe("network");
        // The core is the one file a volume set cannot split by bbox.
        expect(ledger.core.splittable).toBe(false);
        expect(ledger.bands.filter((b) => b.splittable).map((b) => b.band)).toEqual(["coarse", "mid", "fine"]);
        expect(ledger.bands.find((b) => b.contextOnly)!.band).toBe("coarse");
    });

    it("does not warn about a partial coarse cell — that is context, and normal", () => {
        // Selection A: coarse partial, nothing else partial, no holes.
        const ledger = ledgerOf(overA);
        expect(ledger.coverage.partialContextCount).toBe(1);
        expect(ledger.coverage.partialDetailCount).toBe(0);
        expect(ledger.coverage.hasWarnings).toBe(false);
    });

    it("does warn about a partial cell in a band a rider reads detail from", () => {
        const ledger = ledgerOf(overAB);
        expect(ledger.coverage.partialDetailByBand.get("fine")).toEqual(["18/1204/1053"]);
        expect(ledger.coverage.partialDetailCount).toBe(1);
        expect(ledger.coverage.partialContextCount).toBe(1);
        expect(ledger.coverage.hasWarnings).toBe(true);
    });

    it("counts holes separately from partial cells", () => {
        const westOfA = box("w", {
            minLat: A.minLat + 1,
            minLon: A.minLon - 1,
            maxLat: A.maxLat - 1,
            maxLon: A.minLon + 1,
        });
        const ledger = ledgerOf(westOfA);
        expect(ledger.coverage.holesByBand.get("fine")).toEqual(["18/1204/1051"]);
        expect(ledger.coverage.holeCount).toBeGreaterThan(0);
        expect(ledger.coverage.hasWarnings).toBe(true);
    });

    it("says nothing is final while a band's index is missing", () => {
        const partial = { ...ctx, indices: new Map([["fine", indices.get("fine")!]]) };
        const ledger = ledgerFor(
            resolveSelection({ parts: [overA], corridorRadiusM: 0 }, partial),
            exampleCatalog,
            partial.indices,
        );
        expect(ledger.unresolvedBands).toEqual(["coarse", "mid", "network"]);
        expect(ledger.isFinal).toBe(false);
    });

    it("says nothing is final while a region's cell list is still arriving", () => {
        // The half-second in which the card would otherwise state, with total
        // confidence and no holes, that DACH costs 0 B.
        const region: RegionPart = {
            kind: "region",
            id: "region-1",
            name: "Switzerland",
            regionId: "europe/switzerland",
        };
        const ledger = ledgerFor(
            resolveSelection({ parts: [region], corridorRadiusM: 0 }, ctx),
            exampleCatalog,
            indices,
        );
        expect(ledger.totalBytes).toBe(0);
        expect(ledger.unresolvedParts).toEqual(["region-1"]);
        expect(ledger.unresolvedBands).toEqual([]);
        expect(ledger.isFinal).toBe(false);
        // A selection of things that *are* resolved is final, so the flag means
        // something rather than being permanently lit.
        expect(ledgerOf(overA).isFinal).toBe(true);
    });

    it("passes a small selection", () => {
        expect(ledgerOf(overAB).verdict).toEqual({ kind: "ok" });
    });
});

function regionEntry(coreBytes: number): RegionEntry {
    return {
        id: "europe/dach",
        name: "DACH",
        parent: null,
        boundary: { tolerance_udeg: 2000, rings: [] },
        bytes: coreBytes,
        bytes_by_band: { coarse: 0, mid: 0, fine: 0, network: coreBytes },
        cell_count: { network: 1 },
        partial_cell_count: 0,
        cells_url: "/cells.json",
        cells_bytes: 0,
        cells_sha256: "0".repeat(64),
    };
}

describe("the core file's ceiling (OBCA §5.7)", () => {
    it("prices a named region from the root alone — no satellite fetch", () => {
        const entry = exampleCatalog.regions[0];
        const ledger = ledgerForRegion(exampleCatalog, entry);
        expect(ledger.totalBytes).toBe(entry.bytes);
        expect(ledger.core.bytes).toBe(entry.bytes_by_band.network);
        expect(ledger.verdict.kind).toBe("ok");
    });

    it("reports the root's single partial count without warning about it (#1032)", () => {
        // The root cannot split `partial_cell_count` by band, so the coarse rule
        // has nothing to apply itself to until the cell lists load. Warning on
        // it would light the flag for essentially every region ever published —
        // #1025 measured that every coarse cell of a country is partial — and a
        // hatch over every entry in the picker tells a rider nothing. It is a
        // number a card may print, and nothing else, until #1032 splits it.
        const entry = { ...exampleCatalog.regions[0], partial_cell_count: 3 };
        const ledger = ledgerForRegion(exampleCatalog, entry);
        expect(ledger.coverage.unsplitPartialCount).toBe(3);
        expect(ledger.coverage.hasWarnings).toBe(false);
        // …and it is not passed off as a detail count with no cells behind it,
        // which would force a UI to grow a branch for "a count I cannot point
        // at".
        expect(ledger.coverage.partialDetailCount).toBe(0);
        expect(ledger.coverage.partialDetailByBand.size).toBe(0);
        // A real resolution has the split answer and uses the ordinary rule.
        expect(ledgerOf(overAB).coverage.unsplitPartialCount).toBeNull();
        expect(ledgerOf(overAB).coverage.partialDetailCount).toBe(1);
    });

    it("refuses a core whose projected file passes 4 GiB − 1, naming the navigation graph", () => {
        // 3.8 GiB of cells: under the ceiling as bytes, over it as the file
        // §5.7 makes us project (+15 %, 4.37 GiB).
        const verdict = ledgerForRegion(exampleCatalog, regionEntry(Math.round(3.8 * GIB))).verdict;
        expect(verdict.kind).toBe("refuse");
        if (verdict.kind !== "refuse") throw new Error("unreachable");
        expect(verdict.band).toBe("network");
        expect(verdict.limit).toBe(MAX_FILE_BYTES);
        // §5.7: the refusal MUST name the nav graph as the reason and the
        // coverage as the thing to reduce. After §5.1 nothing else is true.
        expect(verdict.message).toMatch(/navigation graph/);
        expect(verdict.message).toMatch(/coverage/);
    });

    it("warns when the projected file passes ≈ 3.5 GiB, still naming the navigation graph", () => {
        // 3.2 GiB of cells → 3.68 GiB projected, over the warn line and well
        // under the ceiling.
        const verdict = ledgerForRegion(exampleCatalog, regionEntry(Math.round(3.2 * GIB))).verdict;
        expect(verdict.kind).toBe("warn");
        if (verdict.kind !== "warn") throw new Error("unreachable");
        expect(verdict.message).toMatch(/navigation graph/);
    });

    it("quotes the figure it judged, so no meter can sit under a line it is refusing", () => {
        // The failure this pins: judging the projected size and quoting the
        // nominal one. Every refusal from 3.48 to 4.0 GiB of cells then said
        // "about 3.x GiB … past the 4 GiB", and the payload's own bytes were
        // under the limit it had just cited — a meter drawn from it would sit
        // comfortably below the wall while the dialog refused to build.
        for (const nominalGiB of [3.0, 3.05, 3.2, 3.48, 3.6, 3.9, 4.0, 5.0]) {
            const nominal = Math.round(nominalGiB * GIB);
            const projected = Math.ceil(nominal * (1 + OVERHEAD_BUDGET));
            const verdict = ledgerForRegion(exampleCatalog, regionEntry(nominal)).verdict;
            if (verdict.kind === "ok") {
                expect(projected).toBeLessThanOrEqual(3.5 * GIB);
                continue;
            }
            // The payload: the judged figure, past the limit it was judged on.
            expect(verdict.nominalBytes).toBe(nominal);
            expect(verdict.projectedBytes).toBe(projected);
            expect(verdict.projectedBytes).toBeGreaterThan(verdict.limit);
            // The sentence: the same two numbers, in that order, spelled the
            // same way. A drift between the message and the payload fails here.
            const quoted = [...verdict.message.matchAll(/([\d.]+) GiB/g)].map((m) => m[1]);
            expect(quoted.slice(0, 2)).toEqual([gib(nominal), gib(projected)]);
            expect(verdict.message).toContain("once assembled");
        }
    });

    it("judges on the pessimistic side of the comparison", () => {
        // 3.1 GiB of real cell bytes is under the 3.5 GiB warn line — but §5.7
        // requires the projection to be an upper bound, and +15 % puts it over.
        const nominal = Math.round(3.1 * GIB);
        expect(nominal).toBeLessThan(3.5 * GIB);
        expect(nominal * (1 + OVERHEAD_BUDGET)).toBeGreaterThan(3.5 * GIB);
        expect(ledgerForRegion(exampleCatalog, regionEntry(nominal)).verdict.kind).toBe("warn");
    });

    it("clears DACH's re-pinned core, barely — the number this design lives on", () => {
        // #1025's re-pin: the DACH core is 3.03 GiB nominal, 3.49 GiB carrying
        // §1.5's +15 %. That lands just under the warn line and well under the
        // ceiling, which is the whole reason the coarse/core band boundary sits
        // where it does.
        expect(ledgerForRegion(exampleCatalog, regionEntry(Math.round(3.03 * GIB))).verdict.kind).toBe("ok");
        expect(ledgerForRegion(exampleCatalog, regionEntry(Math.round(3.05 * GIB))).verdict.kind).toBe("warn");
    });
});

describe("fitsOnCard", () => {
    it("measures against free space, not against any per-file limit", () => {
        // §9/D4: maps are volume sets, so there is no user-visible file-size
        // wall. The only question a rider is asked is whether the card has room.
        const ledger = ledgerOf(overAB);
        const need = Math.ceil(ledger.totalBytes * (1 + OVERHEAD_BUDGET));
        expect(fitsOnCard(ledger, need)).toEqual({ fits: true, shortfallBytes: 0 });
        expect(fitsOnCard(ledger, need - 100)).toEqual({ fits: false, shortfallBytes: 100 });
        expect(fitsOnCard(ledger, 32 * GIB).fits).toBe(true);
    });
});
