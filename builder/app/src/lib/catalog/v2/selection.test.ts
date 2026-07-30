// Composing a selection out of parts, and attributing its bytes back to them.
//
// The scenario is a small corner of the example catalog's grid: three fine cells
// around `18/1204/1052`, their two mid parents, the one coarse cell over all of
// it, and the network cells beside the fine ones. Small enough to state every
// expected id by hand, which is the point — a union computed by a test that
// computes it the same way proves nothing.

import { describe, expect, it } from "vitest";
import { cellSquare, parseCellId } from "./grid";
import { CatalogFormatError } from "./parse";
import { parseRegionCells } from "./satellites";
import {
    emptySelection,
    resolveSelection,
    withCorridorRadius,
    withoutPart,
    withPart,
    type BoxPart,
    type CorridorPart,
    type RegionPart,
    type SelectionContext,
} from "./selection";
import { EXAMPLE_REGION_CELLS, exampleCatalog, fixtureIndices } from "./testdata";

const indices = fixtureIndices(exampleCatalog, {
    coarse: [{ id: "20/0301/0263", bytes: 2088, partial: true }],
    mid: [
        { id: "19/0602/0526", bytes: 1064 },
        { id: "19/0602/0527", bytes: 900 },
    ],
    fine: [
        { id: "18/1204/1052", bytes: 552 },
        { id: "18/1204/1053", bytes: 424, partial: true },
        { id: "18/1205/1052", bytes: 600 },
    ],
    network: [
        { id: "18/1204/1052", bytes: 296 },
        { id: "18/1204/1053", bytes: 168 },
        { id: "18/1205/1052", bytes: 300 },
    ],
});

const swissCells = parseRegionCells(
    EXAMPLE_REGION_CELLS,
    exampleCatalog,
    exampleCatalog.regions.find((r) => r.id === "europe/switzerland")!,
);

const ctx: SelectionContext = {
    catalog: exampleCatalog,
    indices,
    regionCells: new Map([["europe/switzerland", swissCells]]),
};

const A = cellSquare(parseCellId("18/1204/1052"));

/** A box strictly inside cell A, so it names exactly one fine cell. */
const insideA: BoxPart = {
    kind: "box",
    id: "box-1",
    name: "Drawn area",
    box: { minLat: A.minLat + 1000, minLon: A.minLon + 1000, maxLat: A.maxLat - 1000, maxLon: A.maxLon - 1000 },
};

const region: RegionPart = {
    kind: "region",
    id: "region-1",
    name: "Switzerland",
    regionId: "europe/switzerland",
};

describe("selection composition", () => {
    it("adds, replaces by id, and removes", () => {
        let selection = emptySelection(5000);
        selection = withPart(selection, insideA);
        selection = withPart(selection, region);
        expect(selection.parts.map((p) => p.id)).toEqual(["box-1", "region-1"]);
        selection = withPart(selection, { ...insideA, name: "Renamed" });
        expect(selection.parts.map((p) => p.name)).toEqual(["Switzerland", "Renamed"]);
        expect(withoutPart(selection, "region-1").parts.map((p) => p.id)).toEqual(["box-1"]);
        // Nothing mutates: a stale reference is still the selection it was.
        expect(selection.parts).toHaveLength(2);
    });

    it("keeps one global corridor width, never a negative one", () => {
        expect(withCorridorRadius(emptySelection(0), 2500).corridorRadiusM).toBe(2500);
        expect(withCorridorRadius(emptySelection(0), -10).corridorRadiusM).toBe(0);
    });
});

describe("resolveSelection", () => {
    it("resolves a box per band, precisely at fine and generously at coarse", () => {
        const r = resolveSelection({ parts: [insideA], corridorRadiusM: 0 }, ctx);
        expect(r.cellsByBand.get("fine")).toEqual(["18/1204/1052"]);
        expect(r.cellsByBand.get("network")).toEqual(["18/1204/1052"]);
        expect(r.cellsByBand.get("mid")).toEqual(["19/0602/0526"]);
        expect(r.cellsByBand.get("coarse")).toEqual(["20/0301/0263"]);
        expect(r.missingByBand.size).toBe(0);
        expect(r.unresolvedBands).toEqual([]);
    });

    it("reads a region's stored cell list rather than deriving one", () => {
        const r = resolveSelection({ parts: [region], corridorRadiusM: 0 }, ctx);
        expect(r.cellsByBand.get("fine")).toEqual(["18/1204/1052", "18/1204/1053"]);
        expect(r.parts[0].bytes).toBe(2088 + 1064 + 552 + 424 + 296 + 168);
    });

    it("counts a shared cell once in the union and in both parts' gross bytes", () => {
        const r = resolveSelection({ parts: [insideA, region], corridorRadiusM: 0 }, ctx);
        expect(r.cellsByBand.get("fine")).toEqual(["18/1204/1052", "18/1204/1053"]);
        const [box, whole] = r.parts;
        // Gross: everything the part covers, shared or not — so these overlap.
        expect(box.bytes).toBe(2088 + 1064 + 552 + 296);
        // Marginal: what removing it would actually save. The box is wholly
        // inside the region, so removing it saves nothing at all.
        expect(box.marginalBytes).toBe(0);
        expect(whole.marginalBytes).toBe(424 + 168);
        expect(box.marginalBytes + whole.marginalBytes).toBeLessThanOrEqual(
            [...r.cellsByBand].reduce(
                (sum, [band, ids]) =>
                    sum + ids.reduce((s, id) => s + (indices.get(band)!.byId.get(id)?.bytes ?? 0), 0),
                0,
            ),
        );
    });

    it("reports ground with no published cell as a hole rather than dropping it", () => {
        // A box reaching one cell west of A, which this catalog does not publish.
        const westOfA: BoxPart = {
            kind: "box",
            id: "box-2",
            name: "Over the edge",
            box: { minLat: A.minLat + 1000, minLon: A.minLon - 1000, maxLat: A.maxLat - 1000, maxLon: A.minLon + 1000 },
        };
        const r = resolveSelection({ parts: [westOfA], corridorRadiusM: 0 }, ctx);
        expect(r.cellsByBand.get("fine")).toEqual(["18/1204/1052"]);
        expect(r.missingByBand.get("fine")).toEqual(["18/1204/1051"]);
        expect(r.parts[0].missingCount).toBeGreaterThan(0);
    });

    it("buffers a corridor by the selection's global radius", () => {
        const route: CorridorPart = {
            kind: "corridor",
            id: "route-1",
            name: "Day 3 — Basel to Olten",
            points: [
                { lat: (A.minLat + A.maxLat) / 2, lon: A.maxLon - 200 },
                { lat: (A.minLat + A.maxLat) / 2, lon: A.maxLon - 100 },
            ],
        };
        const tight = resolveSelection({ parts: [route], corridorRadiusM: 0 }, ctx);
        expect(tight.cellsByBand.get("fine")).toEqual(["18/1204/1052"]);
        // Widen the one slider and the neighbour joins — with no change to the
        // part itself, which is what "one global width" means.
        const wide = resolveSelection({ parts: [route], corridorRadiusM: 4000 }, ctx);
        expect(wide.cellsByBand.get("fine")).toEqual(["18/1204/1052", "18/1204/1053"]);
    });

    it("contributes nothing for a region whose cell list has not arrived, and says so", () => {
        const pending: SelectionContext = { ...ctx, regionCells: new Map() };
        const r = resolveSelection({ parts: [region], corridorRadiusM: 0 }, pending);
        expect(r.cellsByBand.size).toBe(0);
        expect(r.parts[0].cellCount).toBe(0);
        // 0 B is a perfectly ordinary number, and a card that prints it without
        // this flag is a card that says "DACH — 0 B" a moment before it says
        // "47 GB". Nothing about the total can tell those two apart.
        expect(r.parts[0].pending).toBe(true);
        expect(r.unresolvedParts).toEqual(["region-1"]);

        // Once the list is in hand the flag clears, with no other change.
        const arrived = resolveSelection({ parts: [region], corridorRadiusM: 0 }, ctx);
        expect(arrived.parts[0].pending).toBe(false);
        expect(arrived.unresolvedParts).toEqual([]);
        // Boxes and corridors are never pending: their answer is arithmetic.
        expect(resolveSelection({ parts: [insideA], corridorRadiusM: 0 }, pending).unresolvedParts).toEqual([]);
    });

    it("names the bands whose index is missing instead of pricing without them", () => {
        const partial: SelectionContext = {
            ...ctx,
            indices: new Map([["fine", indices.get("fine")!]]),
        };
        const r = resolveSelection({ parts: [insideA], corridorRadiusM: 0 }, partial);
        expect(r.unresolvedBands).toEqual(["coarse", "mid", "network"]);
        expect(r.cellsByBand.get("fine")).toEqual(["18/1204/1052"]);
    });

    it("refuses a region cell its band does not index — that is not a hole", () => {
        // The distinction the whole coverage drawing rests on. A hole is ground
        // nobody baked: legal, priced at nothing, drawn so the rider can accept
        // it. A region cell list naming a cell the band index does not carry is
        // a *broken publish* — a named cell with no bytes, size or digest — and
        // letting it fall into `missingByBand` would draw it as legal coverage
        // and price the download short.
        const broken: SelectionContext = {
            ...ctx,
            indices: fixtureIndices(exampleCatalog, {
                coarse: [{ id: "20/0301/0263", bytes: 2088, partial: true }],
                mid: [{ id: "19/0602/0526", bytes: 1064 }],
                // …but the region's list names 18/1204/1053 too.
                fine: [{ id: "18/1204/1052", bytes: 552 }],
                network: [
                    { id: "18/1204/1052", bytes: 296 },
                    { id: "18/1204/1053", bytes: 168 },
                ],
            }),
        };
        expect(() => resolveSelection({ parts: [region], corridorRadiusM: 0 }, broken)).toThrow(
            CatalogFormatError,
        );
        expect(() => resolveSelection({ parts: [region], corridorRadiusM: 0 }, broken)).toThrow(
            /18\/1204\/1053 is not in band "fine"'s index/,
        );
        // Composed with other parts, and with the region second: still refused,
        // rather than quietly counted once as a hole.
        expect(() => resolveSelection({ parts: [insideA, region], corridorRadiusM: 0 }, broken)).toThrow(
            CatalogFormatError,
        );
        // A band whose index has not loaded is not evidence of anything, so it
        // is not judged — the check runs again when it arrives.
        const stillLoading: SelectionContext = {
            ...broken,
            indices: new Map([["network", broken.indices.get("network")!]]),
        };
        expect(() => resolveSelection({ parts: [region], corridorRadiusM: 0 }, stillLoading)).not.toThrow();
    });

    it("has nothing to resolve for an empty selection", () => {
        const r = resolveSelection(emptySelection(5000), ctx);
        expect(r.parts).toEqual([]);
        expect(r.cellsByBand.size).toBe(0);
    });
});
