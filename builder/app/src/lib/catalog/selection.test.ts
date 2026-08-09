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
    SelectionResolver,
    withCorridorRadius,
    withoutPart,
    withPart,
    type BoxPart,
    type CorridorPart,
    type RegionPart,
    type SelectionContext,
} from "./selection";
import { EXAMPLE_REGION_CELLS, exampleCatalog, exampleTerrainIndex, fixtureIndices } from "./testdata";

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
}, { fine: [{ start: "18/1204/1055", end: "18/1204/1055" }] });

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
const EMPTY = cellSquare(parseCellId("18/1204/1055"));

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
        // An edit replaces the row where it is. A parts list that reordered
        // itself every time a box was nudged is a list nobody can read.
        selection = withPart(selection, { ...insideA, name: "Renamed" });
        expect(selection.parts.map((p) => p.name)).toEqual(["Renamed", "Switzerland"]);
        expect(selection.parts.map((p) => p.id)).toEqual(["box-1", "region-1"]);
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
        expect(r.cellsByBand.get("fine")).toEqual(["18/1204/1052", "18/1204/1053", "18/1204/1055"]);
        expect(r.parts[0].bytes).toBe(2088 + 1064 + 552 + 424 + 296 + 168);
    });

    it("resolves a lasso ring per band through the same union", () => {
        // A triangle strictly inside cell A: one fine cell, generous coarse
        // coverage — the same consequence-of-cell-size the box test pins.
        const r = resolveSelection(
            {
                parts: [
                    {
                        kind: "lasso",
                        id: "lasso-1",
                        name: "Drawn ring",
                        points: [
                            { lat: A.minLat + 1000, lon: A.minLon + 1000 },
                            { lat: A.minLat + 1000, lon: A.maxLon - 1000 },
                            { lat: A.maxLat - 1000, lon: (A.minLon + A.maxLon) / 2 },
                        ],
                    },
                ],
                corridorRadiusM: 0,
            },
            ctx,
        );
        expect(r.cellsByBand.get("fine")).toEqual(["18/1204/1052"]);
        expect(r.cellsByBand.get("coarse")).toEqual(["20/0301/0263"]);
        expect(r.missingByBand.size).toBe(0);
    });

    it("treats verified-empty ground as zero-byte coverage rather than a hole", () => {
        const emptyBox: BoxPart = {
            kind: "box",
            id: "empty",
            name: "Verified empty",
            box: {
                minLat: EMPTY.minLat + 1,
                minLon: EMPTY.minLon + 1,
                maxLat: EMPTY.maxLat - 1,
                maxLon: EMPTY.maxLon - 1,
            },
        };
        const r = resolveSelection({ parts: [emptyBox], corridorRadiusM: 0 }, ctx);
        expect(r.cellsByBand.get("fine")).toEqual(["18/1204/1055"]);
        expect(r.missingByBand.get("fine")).toBeUndefined();
        expect(r.parts[0].cellsByBand.get("fine")).toEqual(["18/1204/1055"]);
    });

    it("counts a shared cell once in the union and in both parts' gross bytes", () => {
        const r = resolveSelection({ parts: [insideA, region], corridorRadiusM: 0 }, ctx);
        expect(r.cellsByBand.get("fine")).toEqual(["18/1204/1052", "18/1204/1053", "18/1204/1055"]);
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

describe("SelectionResolver", () => {
    const route: CorridorPart = {
        kind: "corridor",
        id: "route-1",
        name: "Day 3",
        points: [
            { lat: (A.minLat + A.maxLat) / 2, lon: A.maxLon - 200 },
            { lat: (A.minLat + A.maxLat) / 2, lon: A.maxLon - 100 },
        ],
    };
    const selection = { parts: [insideA, region, route], corridorRadiusM: 2000 };
    const bands = exampleCatalog.schema.bands.length;

    /** Every list in a resolution, flattened, so two answers can be compared
     *  without caring which object they came from. */
    function shape(r: ReturnType<typeof resolveSelection>) {
        return {
            union: [...r.cellsByBand].map(([b, ids]) => [b, ids]),
            missing: [...r.missingByBand].map(([b, ids]) => [b, ids]),
            parts: r.parts.map((p) => ({
                id: p.part.id,
                bytes: p.bytes,
                marginal: p.marginalBytes,
                cells: [...p.cellsByBand].map(([b, ids]) => [b, ids]),
                pending: p.pending,
            })),
            unresolvedBands: r.unresolvedBands,
            unresolvedParts: r.unresolvedParts,
        };
    }

    it("gives the same answer as resolving from scratch", () => {
        const resolver = new SelectionResolver();
        expect(shape(resolver.resolve(selection, ctx))).toEqual(shape(resolveSelection(selection, ctx)));
        // …and again from the cache, which is the assertion that matters.
        expect(shape(resolver.resolve(selection, ctx))).toEqual(shape(resolveSelection(selection, ctx)));
        expect(resolver.stats.computed).toBe(3 * bands);
        expect(resolver.stats.reused).toBe(3 * bands);
    });

    it("recomputes only the corridors when the one global width moves", () => {
        // The slider case, and the whole reason this class exists: a map of a
        // region and a box recomputing three parts to answer a question about
        // none of them is tens of milliseconds per frame.
        const resolver = new SelectionResolver();
        resolver.resolve(selection, ctx);
        resolver.stats.computed = 0;
        resolver.stats.reused = 0;
        const wider = withCorridorRadius(selection, 6000);
        const memoised = resolver.resolve(wider, ctx);
        expect(resolver.stats.computed).toBe(bands);
        expect(resolver.stats.reused).toBe(2 * bands);
        expect(shape(memoised)).toEqual(shape(resolveSelection(wider, ctx)));
    });

    it("recomputes only the part that was edited", () => {
        const resolver = new SelectionResolver();
        resolver.resolve(selection, ctx);
        resolver.stats.computed = 0;
        resolver.stats.reused = 0;
        const moved = withPart(selection, {
            ...insideA,
            box: { ...insideA.box, maxLon: insideA.box.maxLon + 1_000_000 },
        });
        const memoised = resolver.resolve(moved, ctx);
        expect(resolver.stats.computed).toBe(bands);
        expect(resolver.stats.reused).toBe(2 * bands);
        expect(shape(memoised)).toEqual(shape(resolveSelection(moved, ctx)));
    });

    it("recomputes a band when its index is replaced, and only that band", () => {
        // A cell index arriving (or being re-fetched) changes what "published"
        // means, and an answer computed against the old one is not an answer.
        const resolver = new SelectionResolver();
        resolver.resolve(selection, ctx);
        resolver.stats.computed = 0;
        const withNewFine: SelectionContext = {
            ...ctx,
            indices: new Map([
                ...indices,
                [
                    "fine",
                    // The same cells, re-published at different sizes: a new
                    // document, so every answer computed against the old one is
                    // stale even though the cell ids did not move.
                    fixtureIndices(exampleCatalog, {
                        fine: [
                            { id: "18/1204/1052", bytes: 999 },
                            { id: "18/1204/1053", bytes: 424, partial: true },
                            { id: "18/1205/1052", bytes: 600 },
                        ],
                    }, { fine: [{ start: "18/1204/1055", end: "18/1204/1055" }] }).get("fine")!,
                ],
            ]),
        };
        const r = resolver.resolve(selection, withNewFine);
        expect(resolver.stats.computed).toBe(3);
        expect(shape(r)).toEqual(shape(resolveSelection(selection, withNewFine)));
    });

    it("forgets a part that has been removed, and can be told to forget one", () => {
        const resolver = new SelectionResolver();
        resolver.resolve(selection, ctx);
        expect(resolver.size).toBe(3 * bands);
        resolver.resolve(withoutPart(selection, "route-1"), ctx);
        expect(resolver.size).toBe(2 * bands);

        resolver.stats.computed = 0;
        resolver.invalidate("box-1");
        resolver.resolve(withoutPart(selection, "route-1"), ctx);
        expect(resolver.stats.computed).toBe(bands);

        resolver.invalidateAll();
        expect(resolver.size).toBe(0);
    });

    it("resolves terrain by the same intersect rule, on the terrain grid (EL4, §13.3)", () => {
        const terrain = exampleTerrainIndex();
        // The example store is `2^13` squares; the box below is drawn over the
        // first published one, plus the void square beside it and one the store
        // says nothing about — the three answers §13.3/§13.6 distinguish.
        const first = cellSquare(parseCellId("13/38528/33664"));
        const wide: BoxPart = {
            kind: "box",
            id: "box-t",
            name: "Terrain box",
            box: { minLat: first.minLat + 1, minLon: first.minLon + 1, maxLat: first.maxLat - 1, maxLon: first.maxLon + 2 * 8192 },
        };
        const r = resolveSelection({ parts: [wide], corridorRadiusM: 0 }, { ...ctx, terrain });
        expect(r.terrain.cells).toEqual(["13/38528/33664", "13/38528/33665"]);
        expect(r.terrain.knownEmpty).toEqual(["13/38528/33667"]);
        expect(r.terrain.missing).toEqual(["13/38528/33666"]);
        // §13.1 publishes 548 B per cell in the example.
        expect(r.terrain.bytes).toBe(548 * 2);
    });

    it("takes a region's terrain from its published list, never from its outline", () => {
        const terrain = exampleTerrainIndex();
        const r = resolveSelection({ parts: [region], corridorRadiusM: 0 }, { ...ctx, terrain });
        // Exactly the region satellite's `terrain` array — two objects and the
        // one canonically void square.
        expect([...r.terrain.cells, ...r.terrain.knownEmpty].sort()).toEqual(swissCells.terrain);
        expect(r.terrain.missing).toEqual([]);
    });

    it("names no terrain at all when the catalog publishes none (§13)", () => {
        const r = resolveSelection({ parts: [region], corridorRadiusM: 0 }, ctx);
        expect(r.terrain.cells).toEqual([]);
        expect(r.terrain.bytes).toBe(0);
    });

    it("still refuses a region cell no band index publishes", () => {
        const broken: SelectionContext = {
            ...ctx,
            indices: fixtureIndices(exampleCatalog, {
                coarse: [{ id: "20/0301/0263", bytes: 2088 }],
                mid: [{ id: "19/0602/0526", bytes: 1064 }],
                fine: [{ id: "18/1204/1052", bytes: 552 }],
                network: [{ id: "18/1204/1052", bytes: 296 }],
            }),
        };
        const resolver = new SelectionResolver();
        expect(() => resolver.resolve({ parts: [region], corridorRadiusM: 0 }, broken)).toThrow(
            CatalogFormatError,
        );
    });
});
