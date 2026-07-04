import { describe, expect, it } from "vitest";
import { addLodTier, autoSimplify, editLodTier, exportFile, importFile, newStyleDef, removeCategory, removeLodTier, reorderCategory } from "./edit";
import { ENVELOPE_VERSION } from "./migrations";
import type { PackConfig } from "./model";
import type { WorkingEnvelope } from "./storage.svelte";

function config(): PackConfig {
    return {
        lods: [
            { max_mpp: null, simplify: 120 },
            { max_mpp: 120, simplify: 18 },
            { max_mpp: 18, simplify: 0 },
        ],
        features: {
            highway: {
                motorway: { color: "0xFAA0", min_lod: 0 },
                track: { color: "0xAAA0", min_lod: 2 },
                path: { color: "0xAAA0", min_lod: 1 },
            },
            natural: { land: { color: "0xFFFF", min_lod: 0 } },
        },
        marker: { color: "0xF800" },
    };
}

describe("LOD tier edits", () => {
    it("autoSimplify is the next tier's ceiling, 0 for the finest", () => {
        const cfg = config();
        expect(autoSimplify(cfg, 0)).toBe(120);
        expect(autoSimplify(cfg, 1)).toBe(18);
        expect(autoSimplify(cfg, 2)).toBe(0);
    });

    it("adds a finer tier at half the previous ceiling", () => {
        const cfg = config();
        addLodTier(cfg);
        expect(cfg.lods[3]).toEqual({ max_mpp: 9, simplify: 0 });
        // The old finest tier picks up the pixel-accurate default.
        expect(cfg.lods[2].simplify).toBe(9);
    });

    it("keeps a hand-set simplify on the old finest tier when adding", () => {
        const cfg = config();
        cfg.lods[2].simplify = 5;
        addLodTier(cfg);
        expect(cfg.lods[2].simplify).toBe(5);
    });

    it("a default simplify follows the next tier's ceiling edit", () => {
        const cfg = config();
        editLodTier(cfg, 1, "max_mpp", 100);
        expect(cfg.lods[0].simplify).toBe(100); // was 120 = default, tracks
        editLodTier(cfg, 2, "max_mpp", 20);
        expect(cfg.lods[1].simplify).toBe(20); // was 18 = default, tracks
    });

    it("a hand-set simplify does not follow ceiling edits", () => {
        const cfg = config();
        editLodTier(cfg, 0, "simplify", 40);
        editLodTier(cfg, 1, "max_mpp", 100);
        expect(cfg.lods[0].simplify).toBe(40);
    });

    it("re-defaults a tracking simplify when the next tier is removed", () => {
        const cfg = config();
        removeLodTier(cfg, 1);
        expect(cfg.lods[0].simplify).toBe(18); // now tracks the old tier 2
    });

    it("remaps min_lod on removal: above shifts down, at collapses", () => {
        const cfg = config();
        removeLodTier(cfg, 1);
        expect(cfg.lods.length).toBe(2);
        expect(cfg.features.highway.motorway.min_lod).toBe(0); // below: unchanged
        expect(cfg.features.highway.path.min_lod).toBe(1); // at k: collapses into new tier 1
        expect(cfg.features.highway.track.min_lod).toBe(1); // above: shifted down
    });

    it("pins the new coarsest tier to +inf when tier 0 is removed", () => {
        const cfg = config();
        removeLodTier(cfg, 0);
        expect(cfg.lods[0].max_mpp).toBeNull();
    });

    it("never removes the last tier", () => {
        const cfg = config();
        removeLodTier(cfg, 0);
        removeLodTier(cfg, 0);
        removeLodTier(cfg, 0);
        expect(cfg.lods.length).toBe(1);
    });
});

describe("reorderCategory", () => {
    it("rebuilds keys in the given order", () => {
        const cfg = config();
        reorderCategory(cfg, "highway", ["path", "motorway", "track"]);
        expect(Object.keys(cfg.features.highway)).toEqual(["path", "motorway", "track"]);
    });

    it("keeps entries the order list missed", () => {
        const cfg = config();
        reorderCategory(cfg, "highway", ["track"]);
        expect(Object.keys(cfg.features.highway)).toEqual(["track", "motorway", "path"]);
    });
});

describe("category/type edits", () => {
    it("removeCategory returns the disabled-list keys", () => {
        const cfg = config();
        const keys = removeCategory(cfg, "highway");
        expect(keys.sort()).toEqual(["highway/motorway", "highway/path", "highway/track"]);
        expect(cfg.features.highway).toBeUndefined();
    });

    it("new types default to the finest tier", () => {
        const cfg = config();
        expect(newStyleDef(cfg).min_lod).toBe(2);
    });
});

describe("export / import round-trip", () => {
    const env: WorkingEnvelope = {
        schema_version: ENVELOPE_VERSION,
        based_on: { id: "default", version: 1 },
        modified: true,
        config: config(),
        disabled: ["highway/track"],
    };

    it("export is a bare CLI-shaped config with _meta + disabled", () => {
        const out = JSON.parse(exportFile(env));
        expect(out._meta.app).toBe("obcm-web-builder");
        expect(out._meta.based_on).toEqual({ id: "default", version: 1 });
        expect(Object.keys(out.features)).toEqual(["highway", "natural"]);
        expect(out.disabled).toEqual(["highway/track"]);
    });

    it("round-trips through import with provenance and disabled intact", () => {
        const back = importFile(exportFile(env));
        expect(back).not.toBeNull();
        expect(back!.based_on).toEqual({ id: "default", version: 1 });
        expect(back!.disabled).toEqual(["highway/track"]);
        expect(back!.modified).toBe(true);
        expect(Object.keys(back!.config.features.highway)).toEqual(["motorway", "track", "path"]);
    });

    it("imports a legacy stylesheet (bare config + disabled)", () => {
        const legacy = JSON.stringify({
            lods: [{ max_mpp: null, simplify: 0 }],
            features: { highway: { motorway: { color: "0xFAA0" } } },
            marker: { color: "0xF800" },
            disabled: ["highway/motorway"],
        });
        const env2 = importFile(legacy);
        expect(env2).not.toBeNull();
        expect(env2!.based_on).toBeNull();
        expect(env2!.disabled).toEqual(["highway/motorway"]);
    });

    it("rejects non-config JSON", () => {
        expect(importFile("42")).toBeNull();
        expect(importFile('{"hello": "world"}')).toBeNull();
        expect(importFile("not json")).toBeNull();
    });
});
