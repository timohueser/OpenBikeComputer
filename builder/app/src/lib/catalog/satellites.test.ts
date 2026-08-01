// The satellites, checked against what the root said they would be.
//
// The digest proves the bytes are the ones the root hashed. It does not prove
// the root described them correctly, and it says nothing at all about the two
// facts that decide whether the cells may be assembled together: the schema
// revision they were baked at, and the band they belong to. Those are this
// parser's, and every case below is one of them.

import { describe, expect, it } from "vitest";
import { CatalogFormatError, cellIndexRef, region } from "./manifest";
import { assertRegionCellsIndexed, cellIndexHas, knownEmptyAt, parseCellIndex, parseRegionCells } from "./satellites";
import { EXAMPLE_CELL_INDEX, EXAMPLE_REGION_CELLS, exampleCatalog, fixtureIndex } from "./testdata";

type LooseDoc = any;

const fineRef = cellIndexRef(exampleCatalog, "fine")!;
const swiss = region(exampleCatalog, "europe/switzerland")!;

function mutatedIndex(edit: (d: LooseDoc) => void): string {
    const doc = JSON.parse(EXAMPLE_CELL_INDEX) as LooseDoc;
    edit(doc);
    return JSON.stringify(doc);
}

function mutatedRegion(edit: (d: LooseDoc) => void): string {
    const doc = JSON.parse(EXAMPLE_REGION_CELLS) as LooseDoc;
    edit(doc);
    return JSON.stringify(doc);
}

describe("parseCellIndex", () => {
    it("accepts the index obc-pack actually writes", () => {
        const doc = parseCellIndex(EXAMPLE_CELL_INDEX, exampleCatalog, fineRef);
        expect(doc.band).toBe("fine");
        expect(doc.cells.map((c) => c.id)).toEqual(["18/1204/1052", "18/1204/1053"]);
        expect(doc.cells[0].bytes).toBe(552);
        expect(doc.cells[0].cell).toEqual({ log2: 18, i: 1204, j: 1052 });
        expect(doc.cells[1].sources.map((s) => s.extract_id)).toEqual([
            "europe/germany/baden-wuerttemberg",
            "europe/switzerland",
        ]);
        expect(doc.byId.get("18/1204/1052")).toBe(doc.cells[0]);
        expect(doc.known_empty.map((run) => [run.start, run.end])).toEqual([
            ["18/1204/1055", "18/1204/1055"],
        ]);
        expect(knownEmptyAt(doc, "18/1204/1055")?.sources[0].extract_id).toBe("planet");
        expect(cellIndexHas(doc, "18/1204/1055")).toBe(true);
        expect(cellIndexHas(doc, "18/1204/1054")).toBe(false);
    });

    it.each<[string, (d: LooseDoc) => void]>([
        // §6.3: assembly copies chunk bytes between files, which is only
        // meaningful within one schema revision.
        ["a schema revision the root does not carry", (d) => (d.schema_revision = 6)],
        ["another envelope version", (d) => (d.schema_version = 1)],
        ["a band the root did not pin", (d) => (d.band = "mid")],
        ["a cell of another band's size", (d) => (d.cells[0].id = "19/0602/0526")],
        ["an id that is not canonically padded", (d) => (d.cells[0].id = "18/1204/052")],
        ["cells out of (i, j) order", (d) => d.cells.reverse()],
        ["a duplicated cell", (d) => (d.cells[1].id = d.cells[0].id)],
        ["more cells than the root pinned", (d) => d.cells.push(JSON.parse(JSON.stringify(d.cells[0])))],
        ["a cell with no source extract", (d) => (d.cells[0].sources = [])],
        ["sources out of extract_id order", (d) => d.cells[1].sources.reverse()],
        ["a snapshot date that does not exist", (d) => (d.cells[0].sources[0].snapshot = "2026-02-30")],
        ["a missing partial flag", (d) => delete d.cells[0].partial],
        ["a truncated sha256", (d) => (d.cells[0].sha256 = "abc")],
        // A malformed id is a malformed *document*, and must arrive as one: the
        // grid's own `GridError` would sail straight past the handler that
        // catches a bad catalog and land as a blank screen instead.
        ["an id the grid does not admit", (d) => (d.cells[0].id = "18/2048/1052")],
        ["an id that is not three numbers", (d) => (d.cells[0].id = "fine/1204/1052")],
        ["a url that is neither absolute nor root-relative", (d) => (d.cells[0].url = "cells/x.obcm")],
        ["a known-empty range in another band size", (d) => (d.known_empty[0].start = "19/0602/0527")],
        ["a known-empty range crossing rows", (d) => (d.known_empty[0].end = "18/1205/1055")],
        ["a backwards known-empty range", (d) => (d.known_empty[0].end = "18/1204/1053")],
        ["a known-empty range with a noncanonical id", (d) => (d.known_empty[0].start = "18/1204/054")],
        ["a known-empty range overlapping an artifact", (d) => (d.known_empty[0].start = "18/1204/1053")],
        ["a known-empty range with no source", (d) => (d.known_empty[0].sources = [])],
        ["a known-empty count that disagrees with the root", (d) => (d.known_empty[0].end = "18/1204/1056")],
    ])("rejects %s", (_what, edit) => {
        expect(() => parseCellIndex(mutatedIndex(edit), exampleCatalog, fineRef)).toThrow(CatalogFormatError);
    });

    it("accepts an older v2 satellite without additive known-empty coverage", () => {
        const olderRoot = JSON.parse(JSON.stringify(exampleCatalog)) as LooseDoc;
        olderRoot.cell_index.find((ref: LooseDoc) => ref.band === "fine").known_empty_count = 0;
        const body = mutatedIndex((d) => delete d.known_empty);
        const doc = parseCellIndex(body, olderRoot, olderRoot.cell_index.find((ref: LooseDoc) => ref.band === "fine"));
        expect(doc.known_empty).toEqual([]);
        expect(doc.emptyByRow.size).toBe(0);
    });

    it("rejects overlapping, out-of-order, and needlessly split ranges", () => {
        const base = (edit: (d: LooseDoc) => void) => {
            const catalog = JSON.parse(JSON.stringify(exampleCatalog)) as LooseDoc;
            catalog.cell_index.find((ref: LooseDoc) => ref.band === "fine").known_empty_count = 2;
            return () => parseCellIndex(mutatedIndex(edit), catalog, catalog.cell_index.find((ref: LooseDoc) => ref.band === "fine"));
        };
        expect(
            base((d) => d.known_empty.push({ ...d.known_empty[0], start: "18/1204/1055", end: "18/1204/1055" })),
        ).toThrow(CatalogFormatError);
        expect(
            base((d) => d.known_empty.push({ ...d.known_empty[0], start: "18/1203/1056", end: "18/1203/1056" })),
        ).toThrow(CatalogFormatError);
        expect(
            base((d) => d.known_empty.push({ ...d.known_empty[0], start: "18/1204/1056", end: "18/1204/1056" })),
        ).toThrow(/must be merged/);
    });
});

describe("parseRegionCells", () => {
    it("accepts the list obc-pack actually writes", () => {
        const doc = parseRegionCells(EXAMPLE_REGION_CELLS, exampleCatalog, swiss);
        expect(doc.region_id).toBe("europe/switzerland");
        expect(Object.keys(doc.cells).sort()).toEqual(["coarse", "fine", "mid", "network"]);
        expect(doc.cells.fine).toEqual(["18/1204/1052", "18/1204/1053", "18/1204/1055"]);
    });

    it.each<[string, (d: LooseDoc) => void]>([
        ["another region's list", (d) => (d.region_id = "europe/switzerland/basel-stadt")],
        ["a schema revision the root does not carry", (d) => (d.schema_revision = 6)],
        ["a band the schema lacks", (d) => (d.cells.vivid = [])],
        ["a cell of another band's size in a band", (d) => (d.cells.fine[0] = "19/0602/0526")],
        ["ids out of order", (d) => d.cells.fine.reverse()],
        ["a duplicated id", (d) => (d.cells.fine[1] = d.cells.fine[0])],
        // The root priced this region from these counts; a disagreement means
        // the price shown before the download is not the download's price.
        ["a band with more cells than the root priced", (d) => d.cells.mid.push("19/0602/0527")],
        ["a priced band that is absent", (d) => delete d.cells.mid],
        ["an id the grid does not admit", (d) => (d.cells.fine[0] = "18/9999/1052")],
    ])("rejects %s", (_what, edit) => {
        expect(() => parseRegionCells(mutatedRegion(edit), exampleCatalog, swiss)).toThrow(CatalogFormatError);
    });
});

describe("assertRegionCellsIndexed", () => {
    const doc = parseRegionCells(EXAMPLE_REGION_CELLS, exampleCatalog, swiss);

    it("passes when every named cell is in its band's index", () => {
        const indices = new Map([
            [
                "fine",
                fixtureIndex(exampleCatalog, "fine", [
                    { id: "18/1204/1052", bytes: 552 },
                    { id: "18/1204/1053", bytes: 424 },
                ], [{ start: "18/1204/1055", end: "18/1204/1055" }]),
            ],
        ]);
        expect(() => assertRegionCellsIndexed(doc, indices)).not.toThrow();
    });

    it("rejects a named cell with no index entry — a broken publish, not a hole", () => {
        const indices = new Map([
            ["fine", fixtureIndex(exampleCatalog, "fine", [{ id: "18/1204/1052", bytes: 552 }])],
        ]);
        expect(() => assertRegionCellsIndexed(doc, indices)).toThrow(/18\/1204\/1053/);
    });

    it("accepts a named cell covered by a known-empty range", () => {
        const indices = new Map([
            [
                "fine",
                fixtureIndex(
                    exampleCatalog,
                    "fine",
                    [
                        { id: "18/1204/1052", bytes: 552 },
                        { id: "18/1204/1053", bytes: 424 },
                    ],
                    [{ start: "18/1204/1055", end: "18/1204/1055" }],
                ),
            ],
        ]);
        expect(() => assertRegionCellsIndexed(doc, indices)).not.toThrow();
    });

    it("says nothing about bands whose index is not loaded", () => {
        expect(() => assertRegionCellsIndexed(doc, new Map())).not.toThrow();
    });
});
