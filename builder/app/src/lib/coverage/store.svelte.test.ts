// The coverage store, over the real example catalog and the real parsers.
//
// These are the selection-to-UI adapters #1038 adds on top of the (already
// tested) v2 arithmetic: part lifecycle and naming, the pending/final story a
// summary card depends on, the refusals that must arrive as sentences rather
// than crashes, and — measured, not assumed — the resolver reuse that keeps the
// corridor slider smooth.

import { describe, expect, it } from "vitest";
import { CatalogV2Client } from "../catalog/v2/client";
import { parseRegionCells } from "../catalog/v2/satellites";
import { EXAMPLE_REGION_CELLS, EXAMPLE_ROOT, exampleCatalog, fixtureIndices } from "../catalog/v2/testdata";
import { CoverageStore } from "./store.svelte";
import { degreesToUbox } from "./shape";

const ROOT_URL = "https://maps.example.org/catalog/v2/catalog.json";

/** No network anywhere: the constructor's index load fails quietly and the test
 *  injects documents built through the real parsers. */
const offline = (async () => new Response("offline", { status: 503 })) as unknown as typeof fetch;

/** The example's four bands, with cell bytes that sum to the root's own
 *  region pricing (Switzerland 4 592 across 6 cells). */
function indices() {
    return fixtureIndices(exampleCatalog, {
        coarse: [{ id: "20/0301/0263", bytes: 2088, partial: true }],
        mid: [{ id: "19/0602/0526", bytes: 1064 }],
        fine: [
            { id: "18/1204/1052", bytes: 552 },
            { id: "18/1204/1053", bytes: 424 },
        ],
        network: [
            { id: "18/1204/1052", bytes: 296 },
            { id: "18/1204/1053", bytes: 168 },
        ],
    });
}

function makeStore(): CoverageStore {
    const client = CatalogV2Client.fromBody(EXAMPLE_ROOT, ROOT_URL, { fetchImpl: offline });
    const store = new CoverageStore(client, EXAMPLE_ROOT);
    store.indices = indices();
    return store;
}

function withSwissList(store: CoverageStore): void {
    const entry = exampleCatalog.regions.find((r) => r.id === "europe/switzerland")!;
    store.regionCells = new Map([
        ["europe/switzerland", parseRegionCells(EXAMPLE_REGION_CELLS, exampleCatalog, entry)],
    ]);
}

/** A box inside the example's one fully published column of cells. */
const PUBLISHED_BOX = degreesToUbox(47.2, 7.35, 47.4, 7.6);

describe("region parts", () => {
    it("prices as pending until the cell list arrives, then final at the root's own number", () => {
        const store = makeStore();
        store.addRegion("europe/switzerland");
        expect(store.ledger?.isFinal).toBe(false);
        expect(store.resolution?.parts[0].pending).toBe(true);

        withSwissList(store);
        const ledger = store.ledger!;
        expect(ledger.isFinal).toBe(true);
        expect(ledger.totalBytes).toBe(4592);
        expect(ledger.cellCount).toBe(6);
    });

    it("adds a region once — a second click is a no-op, not a duplicate row", () => {
        const store = makeStore();
        store.addRegion("europe/switzerland");
        store.addRegion("europe/switzerland");
        expect(store.selection.parts).toHaveLength(1);
        expect(store.hasRegion("europe/switzerland")).toBe(true);
    });

    it("names the part what the catalog names the region", () => {
        const store = makeStore();
        store.addRegion("europe/switzerland");
        expect(store.selection.parts[0].name).toBe("Switzerland");
    });
});

describe("box parts", () => {
    it("names a box after the smallest region under its centre", () => {
        const store = makeStore();
        // Centred inside both regions' boundaries; Basel-Stadt's is the smaller.
        store.addBox(degreesToUbox(47.51, 7.52, 47.59, 7.68));
        expect(store.selection.parts[0].name).toBe("Box — Basel-Stadt");
        // A box off the shelf entirely falls back to a counter.
        store.addBox(degreesToUbox(10.0, 10.0, 10.2, 10.2));
        expect(store.selection.parts[1].name).toBe("Box 2");
    });

    it("prices a box by its published cells, and overlap costs nothing", () => {
        const store = makeStore();
        withSwissList(store);
        store.addRegion("europe/switzerland");
        store.addBox(PUBLISHED_BOX);
        const [region, box] = store.resolution!.parts;
        // The box's cells are all inside the region: gross bytes are honest
        // per part, the union pays once, and removing the box frees nothing.
        expect(box.bytes).toBe(4000);
        expect(box.marginalBytes).toBe(0);
        expect(region.marginalBytes).toBe(592);
        expect(store.ledger!.totalBytes).toBe(4592);
    });

    it("refuses a box past the enumeration ceiling as a sentence, not a crash", () => {
        const store = makeStore();
        store.addBox(degreesToUbox(-60, -170, 60, 170));
        expect(store.selection.parts).toHaveLength(0);
        expect(store.boxError).toMatch(/smaller one/);
        // …and the ledger is still alive.
        expect(store.ledger).not.toBeNull();

        store.addBox(PUBLISHED_BOX);
        expect(store.boxError).toBeNull();
        expect(store.selection.parts).toHaveLength(1);
    });

    it("shows ground with no published cell as holes, and can point the map at them", () => {
        const store = makeStore();
        store.addBox(degreesToUbox(46.0, 6.0, 46.1, 6.1));
        const ledger = store.ledger!;
        expect(ledger.coverage.holeCount).toBeGreaterThan(0);
        expect(store.focus).toBeNull();
        store.focusWarnings("hole");
        expect(store.focus).not.toBeNull();
        expect(store.focus!.minLat).toBeLessThanOrEqual(46_000_000);
        expect(store.focus!.maxLat).toBeGreaterThanOrEqual(46_100_000);
    });
});

describe("corridor previews", () => {
    const route = [
        { lat: 47_200_000, lon: 7_400_000 },
        { lat: 47_350_000, lon: 7_550_000 },
    ];

    it("commits one part per route, so the ground between two routes stays a hole", () => {
        const store = makeStore();
        store.previewParts = [
            store.makePreviewPart("r1", "Morning Loop", route),
            store.makePreviewPart("r2", "Evening Loop", route),
        ];
        store.commitPreview();
        expect(store.previewParts).toHaveLength(0);
        expect(store.selection.parts.map((p) => p.kind)).toEqual(["corridor", "corridor"]);
        expect(store.selection.parts.map((p) => p.name)).toEqual(["Morning Loop", "Evening Loop"]);
    });

    it("prices what a candidate adds on top of the committed selection", () => {
        const store = makeStore();
        withSwissList(store);
        store.previewParts = [store.makePreviewPart("r1", "Loop", route)];
        const alone = store.previewSummary(1)!;
        expect(alone.addsBytes).toBeGreaterThan(0);

        // With Switzerland already in the map, the same corridor adds nothing —
        // its cells are already paid for.
        store.addRegion("europe/switzerland");
        const covered = store.previewSummary(1)!;
        expect(covered.addsBytes).toBe(0);
        expect(covered.addsCells).toBe(0);
    });

    it("reuses every non-corridor answer when the width slider moves", () => {
        const store = makeStore();
        withSwissList(store);
        store.addRegion("europe/switzerland");
        store.addCorridor("Loop", route);
        void store.ledger; // settle the cache

        const before = { ...store.resolverStats };
        store.setCorridorRadius(25_000);
        void store.ledger;
        const after = store.resolverStats;
        const bands = exampleCatalog.schema.bands.length;
        // The radius is global, so only the corridor part recomputes — one
        // answer per band — and the region's four are cache hits. This is the
        // measurement behind "the slider re-buffers live and stays smooth".
        expect(after.computed - before.computed).toBe(bands);
        expect(after.reused - before.reused).toBe(bands);
    });
});

describe("refusals arrive as sentences", () => {
    it("reports a broken publish as a resolution error, never as a hole", () => {
        const store = makeStore();
        const entry = exampleCatalog.regions.find((r) => r.id === "europe/switzerland")!;
        const doc = parseRegionCells(EXAMPLE_REGION_CELLS, exampleCatalog, entry);
        store.regionCells = new Map([["europe/switzerland", doc]]);
        store.addRegion("europe/switzerland");
        expect(store.resolutionError).toBeNull();

        // The same catalog with the fine index thinned by one cell the region
        // list still names: both documents valid, the pair broken.
        store.indices = fixtureIndices(exampleCatalog, {
            coarse: [{ id: "20/0301/0263", bytes: 2088, partial: true }],
            mid: [{ id: "19/0602/0526", bytes: 1064 }],
            fine: [{ id: "18/1204/1052", bytes: 552 }],
            network: [
                { id: "18/1204/1052", bytes: 296 },
                { id: "18/1204/1053", bytes: 168 },
            ],
        });
        expect(store.resolution).toBeNull();
        expect(store.resolutionError).toMatch(/18\/1204\/1053/);
        expect(store.ledger).toBeNull();
    });
});
