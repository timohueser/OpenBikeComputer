// The size ledger: real bytes, and what deserves a warning.
//
// Two findings are pinned here because losing either costs a user something
// concrete: a price that overstates a selection by half (the area-times-density
// trap), and a coverage warning that hatches an entire country because its coarse
// cells are — normally, unavoidably — partial.

import { describe, expect, it } from "vitest";
import { ledgerFor, ledgerForRegion } from "./ledger";
import { resolveSelection, type BoxPart, type RegionPart, type SelectionContext } from "./selection";
import { cellSquare, parseCellId } from "./grid";
import { exampleCatalog, fixtureIndices } from "./testdata";

const indices = fixtureIndices(exampleCatalog, {
    // Partial coarse cells are the normal state at country scale: a 2^20 cell is
    // ≈ 9 100 km², and no such cell is fully interior to Switzerland.
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
        // Border cells carry the neighbour's overhang, so cell squares cover 1.5–1.8×
        // a region's ground. Anything derived from area × density would be wrong by
        // that much; these are the catalog's own numbers.
        const ledger = ledgerOf(overAB);
        expect(ledger.totalBytes).toBe(2088 + 1064 + 552 + 424 + 296 + 168);
        expect(ledger.cellCount).toBe(6);
        // The two fine cells differ by a third — an average-times-count price
        // would land somewhere else entirely.
        expect(ledger.bands.find((b) => b.band === "fine")!.bytes).toBe(552 + 424);
    });

    it("splits the total by band, and names the core band's line", () => {
        const ledger = ledgerOf(overA);
        expect(Object.fromEntries(ledger.bands.map((b) => [b.band, b.bytes]))).toEqual({
            coarse: 2088,
            mid: 1064,
            fine: 552,
            network: 296,
        });
        expect(ledger.core.band).toBe("network");
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

    it("prices known-empty coverage at zero without reporting a fine-band hole", () => {
        const empty = cellSquare(parseCellId("18/1204/1055"));
        const emptyBox = box("empty", {
            minLat: empty.minLat + 1,
            minLon: empty.minLon + 1,
            maxLat: empty.maxLat - 1,
            maxLon: empty.maxLon - 1,
        });
        const emptyIndices = fixtureIndices(
            exampleCatalog,
            {
                coarse: [{ id: "20/0301/0263", bytes: 2088, partial: true }],
                mid: [{ id: "19/0602/0527", bytes: 900 }],
                fine: [],
                network: [],
            },
            { fine: [{ start: "18/1204/1055", end: "18/1204/1055" }] },
        );
        const resolution = resolveSelection(
            { parts: [emptyBox], corridorRadiusM: 0 },
            { catalog: exampleCatalog, indices: emptyIndices, regionCells: new Map() },
        );
        const ledger = ledgerFor(resolution, exampleCatalog, emptyIndices);
        const fine = ledger.bands.find((band) => band.band === "fine")!;
        expect(fine.cellCount).toBe(1);
        expect(fine.bytes).toBe(0);
        expect(fine.missingCells).toEqual([]);
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
});

describe("pricing a region from the root (OBCC §6)", () => {
    it("prices a named region from the root alone — no satellite fetch", () => {
        const entry = exampleCatalog.regions[0];
        const ledger = ledgerForRegion(exampleCatalog, entry);
            // The raster is priced in the root too, so a hover shows the whole download —
            // and it stays a **separate line**, because a rider may take the map without
            // it and `bytes_by_band` deliberately excludes it.
        expect(ledger.terrain?.bytes).toBe(entry.terrain?.bytes);
        expect(ledger.totalBytes).toBe(entry.bytes + entry.terrain!.bytes);
        expect(Object.values(entry.bytes_by_band).reduce((a, b) => a + b, 0)).toBe(entry.bytes);
        expect(ledger.core.bytes).toBe(entry.bytes_by_band.network);
    });

    it("shows the catalog's attribution and never a hard-coded one (§13.5)", () => {
        const ledger = ledgerForRegion(exampleCatalog, exampleCatalog.regions[0]);
        expect(ledger.terrain?.attribution).toBe(exampleCatalog.terrain!.attribution);
        expect(ledger.terrain?.attribution).toMatch(/Copernicus/);
            // The licence obligation covers every listed reference too, so the ledger carries
            // them all the way to the card rather than leaving them in the root document.
        expect(ledger.terrain?.references).toEqual(exampleCatalog.terrain!.references);
        expect(ledger.terrain?.references.map((r) => r.attribution)).toContain("© swisstopo");
    });

    it("prices a terrain-less catalog with no elevation line at all (§13)", () => {
        const plain = { ...exampleCatalog, terrain: null };
        const ledger = ledgerForRegion(plain, plain.regions[0]);
        expect(ledger.terrain).toBeNull();
        expect(ledger.totalBytes).toBe(plain.regions[0].bytes);
    });

    it("applies the coarse-context rule to the root's per-band partial counts (#1032)", () => {
        const entry = {
            ...exampleCatalog.regions[0],
            partial_cell_count_by_band: { coarse: 1, fine: 1, mid: 0, network: 0 },
        };
        const ledger = ledgerForRegion(exampleCatalog, entry);
        expect(ledger.coverage.partialContextCount).toBe(1);
        expect(ledger.coverage.partialDetailCount).toBe(1);
        expect(ledger.coverage.hasWarnings).toBe(true);
        // The root has counts, not ids: warn in the summary now, hatch only
        // once the pinned cell list and indexes identify the cells.
        expect(ledger.coverage.partialDetailByBand.size).toBe(0);
    });
});
