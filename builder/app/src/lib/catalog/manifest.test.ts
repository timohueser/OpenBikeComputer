// The current root, whole or not at all.
//
// The first test is the one that matters most: the root the real generator
// writes must parse. Everything after it is a document that must be refused *in
// full* — and each case is a specific MUST from `OBCC_Spec.md`, not a
// generic malformed-JSON exercise. A catalog that priced a selection wrongly, or
// described a band table that cannot become a volume set, is not a catalog worth
// half-reading.

import { describe, expect, it } from "vitest";
import { CatalogFormatError, coreBand, parseRoot } from "./manifest";
import { EXAMPLE_ROOT } from "./testdata";

type LooseDoc = any; // deliberately loose: these cases break the shape on purpose

function mutated(edit: (doc: LooseDoc) => void): string {
    const doc = JSON.parse(EXAMPLE_ROOT) as LooseDoc;
    edit(doc);
    return JSON.stringify(doc);
}

describe("parseRoot", () => {
    it("accepts the root obc-pack actually writes", () => {
        const catalog = parseRoot(EXAMPLE_ROOT);
        expect(catalog.schema_version).toBe(2);
        expect(catalog.schema.id).toBe("bikepacking");
        expect(catalog.schema.revision).toBe(7);
        expect(catalog.schema.obcm_version).toBe(11);
        expect(catalog.schema.bands.map((b) => b.id)).toEqual(["coarse", "mid", "fine", "network"]);
        expect(catalog.skins.map((s) => s.id)).toEqual(["contrast", "default"]);
        expect(catalog.skins[0].preview).toBeNull();
        expect(catalog.skins[1].preview).toMatchObject({
            url: "https://maps.example.org/catalog/previews/default.png",
            bytes: 19,
        });
        expect(catalog.regions.map((r) => r.id)).toEqual([
            "europe/switzerland",
            "europe/switzerland/basel-stadt",
        ]);
        expect(catalog.cell_index.map((r) => r.band)).toEqual(["coarse", "mid", "fine", "network"]);
    });

    it("names the one band whose bytes become the core file", () => {
        const catalog = parseRoot(EXAMPLE_ROOT);
        const core = coreBand(catalog);
        expect(core.id).toBe("network");
        expect(core.lods).toEqual([]);
        expect(core.sections).toEqual(["nav", "poi"]);
    });

    it("ignores fields it does not recognise", () => {
        const doc = mutated((d) => {
            d.mirrors = ["https://mirror.example.org/"];
            d.regions[0].population = 8_700_000;
        });
        expect(parseRoot(doc).regions).toHaveLength(2);
    });

    it.each<[string, (d: LooseDoc) => void]>([
        ["an envelope version it does not implement", (d) => (d.schema_version = 3)],
        ["a missing schema_version", (d) => delete d.schema_version],
        ["a missing schema", (d) => delete d.schema],
        ["a timestamp in another spelling", (d) => (d.generated_at = "2026-07-30T09:00:00.5Z")],
        ["a date that does not exist", (d) => (d.generated_at = "2026-02-30T09:00:00Z")],
    ])("rejects %s", (_what, edit) => {
        expect(() => parseRoot(mutated(edit))).toThrow(CatalogFormatError);
    });

    describe("the band table (OBCA §1.2 partition, §5.1 roles)", () => {
        it.each<[string, (d: LooseDoc) => void]>([
            ["a LOD in two bands", (d) => d.schema.bands[1].lods.push(0)],
            ["a LOD in no band", (d) => (d.schema.bands[0].lods = [])],
            ["the nav section in no band", (d) => (d.schema.bands[3].sections = ["poi"])],
            ["the nav section in two bands", (d) => (d.schema.bands[1].sections = ["nav"])],
            ["two core bands", (d) => (d.schema.bands[2].role = "core")],
            ["no core band", (d) => (d.schema.bands[3].role = "geometry")],
            // Geometry in the one file a volume set cannot split by bbox.
            [
                "a core band carrying a LOD",
                (d) => {
                    d.schema.bands[3].lods = [2];
                    d.schema.bands[2].lods = [];
                },
            ],
            ["two coarse bands", (d) => (d.schema.bands[1].role = "coarse")],
            ["a geometry band carrying a section", (d) => (d.schema.bands[1].sections = ["poi"])],
            ["a band listed twice", (d) => (d.schema.bands[1].id = "coarse")],
            ["a cell size the grid does not admit", (d) => (d.schema.bands[0].cell_log2 = 30)],
            ["a ladder rung claiming a band that does not carry it", (d) => (d.schema.lods[0].band = "mid")],
            ["a ladder rung naming a band that does not exist", (d) => (d.schema.lods[0].band = "vivid")],
            ["a ladder that skips an index", (d) => (d.schema.lods[1].index = 5)],
        ])("rejects %s", (_what, edit) => {
            expect(() => parseRoot(mutated(edit))).toThrow(CatalogFormatError);
        });
    });

    describe("the grid constants", () => {
        it.each<[string, (d: LooseDoc) => void]>([
            ["another origin", (d) => (d.schema.grid.origin_udeg = -90_000_000)],
            ["another world side", (d) => (d.schema.grid.world_side_udeg = 360_000_000)],
        ])("rejects %s — this client's cell arithmetic is built on OBCA §1.1's", (_what, edit) => {
            expect(() => parseRoot(mutated(edit))).toThrow(CatalogFormatError);
        });
    });

    describe("skins", () => {
        it.each<[string, (d: LooseDoc) => void]>([
            // §5: a missing style ships a map with an invisible layer…
            ["a skin missing a feature type", (d) => d.skins[0].styles.pop()],
            // …and an unknown one is a stale skin claiming a layer that is gone.
            ["a skin naming a feature type the schema lacks", (d) => (d.skins[0].styles[0].feature_type = "highway.moonbase")],
            ["a skin styling one feature type twice", (d) => (d.skins[0].styles[1].feature_type = d.skins[0].styles[0].feature_type)],
            ["a priority outside 1..4", (d) => (d.skins[0].styles[0].priority = 7)],
            ["a colour outside RGB565", (d) => (d.skins[0].styles[0].color = 70_000)],
            ["two skins sharing an id", (d) => (d.skins[1].id = d.skins[0].id)],
            ["no skins at all", (d) => (d.skins = [])],
            ["a preview URL that cannot be resolved", (d) => (d.skins[1].preview.url = "default.png")],
            ["a truncated preview digest", (d) => (d.skins[1].preview.sha256 = "abc")],
            ["a negative preview size", (d) => (d.skins[1].preview.bytes = -1)],
        ])("rejects %s", (_what, edit) => {
            expect(() => parseRoot(mutated(edit))).toThrow(CatalogFormatError);
        });

    });

    describe("regions", () => {
        it.each<[string, (d: LooseDoc) => void]>([
            // §6: the split is the per-file price. A split that does not add
            // up is a price that is wrong for at least one file.
            ["bytes_by_band that does not sum to bytes", (d) => (d.regions[0].bytes_by_band.fine += 1)],
            ["a band in bytes_by_band that the schema lacks", (d) => (d.regions[0].bytes_by_band.vivid = 0)],
            ["a band in cell_count that the schema lacks", (d) => (d.regions[0].cell_count.vivid = 1)],
            ["more partial cells than cells", (d) => (d.regions[0].partial_cell_count = 99)],
            ["two regions sharing an id", (d) => (d.regions[1].id = d.regions[0].id)],
            ["a parent that is not in the catalog", (d) => (d.regions[1].parent = "europe/atlantis")],
            ["a truncated cells_sha256", (d) => (d.regions[0].cells_sha256 = "abc")],
            ["a cells_url that is neither absolute nor root-relative", (d) => (d.regions[0].cells_url = "cells.json")],
            ["an unclosed boundary ring", (d) => d.regions[0].boundary.rings[0].pop()],
            ["a boundary point that is not [lat, lon]", (d) => (d.regions[0].boundary.rings[0][0] = [1, 2, 3])],
            ["a fractional boundary coordinate", (d) => (d.regions[0].boundary.rings[0][0] = [45.5, 5.9])],
        ])("rejects %s", (_what, edit) => {
            expect(() => parseRoot(mutated(edit))).toThrow(CatalogFormatError);
        });

        it("accepts a region with no parent", () => {
            expect(parseRoot(EXAMPLE_ROOT).regions[0].parent).toBeNull();
        });
    });

    describe("cell index refs", () => {
        it.each<[string, (d: LooseDoc) => void]>([
            ["a band with no cell index", (d) => d.cell_index.pop()],
            ["two indices for one band", (d) => (d.cell_index[1].band = "coarse")],
            ["an index naming a band the schema lacks", (d) => (d.cell_index[0].band = "vivid")],
            ["a cell_log2 that disagrees with its band", (d) => (d.cell_index[0].cell_log2 = 18)],
            ["entries out of descending cell_log2 order", (d) => d.cell_index.reverse()],
            ["a truncated sha256", (d) => (d.cell_index[0].sha256 = "abc")],
        ])("rejects %s", (_what, edit) => {
            expect(() => parseRoot(mutated(edit))).toThrow(CatalogFormatError);
        });
    });

    it("rejects a truncated body rather than reading its prefix", () => {
        const half = EXAMPLE_ROOT.slice(0, Math.floor(EXAMPLE_ROOT.length / 2));
        expect(() => parseRoot(half)).toThrow(CatalogFormatError);
    });

    describe("§3's ordering, which is part of the document", () => {
        it.each<[string, (d: LooseDoc) => void]>([
            ["skins out of id order", (d) => d.skins.reverse()],
            ["regions out of id order", (d) => d.regions.reverse()],
        ])("rejects %s", (_what, edit) => {
            // The cell index's ordering was already checked; these two are the
            // same rule from the same section, and a stated order that is not
            // kept is an order nothing downstream may rely on.
            expect(() => parseRoot(mutated(edit))).toThrow(CatalogFormatError);
        });
    });

    it("accepts a coarsest LOD that simply omits max_mpp", () => {
        // The JSON schema requires `index` and `band` and nothing else; a parser
        // stricter than the schema refuses documents the generator is entitled
        // to write, and "+inf" is exactly what an absent bound means.
        const doc = mutated((d) => delete d.schema.lods[0].max_mpp);
        expect(parseRoot(doc).schema.lods[0].max_mpp).toBeNull();
    });

    it("keeps a band called `constructor` out of the prototype", () => {
        // A kebab id, therefore legal, therefore something a bakery could mint
        // one day. On an ordinary object literal `bytes_by_band["constructor"]`
        // answers with a function for a band the document never mentioned —
        // and `?? 0` does not catch a function, so the price becomes NaN.
        const catalog = parseRoot(EXAMPLE_ROOT);
        const region = catalog.regions[0];
        expect(region.bytes_by_band.constructor).toBeUndefined();
        expect(region.cell_count.constructor).toBeUndefined();
        expect(Object.getPrototypeOf(region.bytes_by_band)).toBeNull();
    });
});
