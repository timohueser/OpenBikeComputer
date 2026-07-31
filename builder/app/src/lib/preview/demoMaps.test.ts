/**
 * The preview index, checked against the document `builder/bake-previews.sh` actually writes.
 *
 * The interesting assertion is the last one: every bakeable config in `builder/presets/` has a demo
 * map in the index, and the file it names exists. That is #899's "a new style gets a preview from a
 * single bake run" — a config added without re-running the bake, or a bake run that half-failed,
 * fails here rather than showing a visitor an empty card.
 *
 * Since #1036 that directory holds one bakeable document (`schema.json`, id `bikepacking`) plus a
 * `skins/` subdirectory. Ids come from `_meta.id`, not from filenames, because the id is what the
 * hosts serve and therefore what a card looks its map up by.
 */

import { readFileSync, existsSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { parsePreviewIndex, PREVIEW_SCHEMA_VERSION } from "./demoMaps";
import { presetTagline, presetsWithCopy } from "./copy";

const PREVIEW_DIR = join(import.meta.dirname, "../../../public/preview");
const PRESETS_DIR = join(import.meta.dirname, "../../../../presets");

/** The `_meta.id` of every bakeable config in `builder/presets/` — skins/ is not a `*.json`. */
function presetIds(): string[] {
    return readdirSync(PRESETS_DIR)
        .filter((f) => f.endsWith(".json"))
        .map((f) => JSON.parse(readFileSync(join(PRESETS_DIR, f), "utf8"))._meta.id as string);
}

/** A preset's `_meta`, by the id it publishes. */
function presetMeta(id: string): { version: number } {
    const file = readdirSync(PRESETS_DIR)
        .filter((f) => f.endsWith(".json"))
        .find((f) => JSON.parse(readFileSync(join(PRESETS_DIR, f), "utf8"))._meta.id === id);
    if (!file) throw new Error(`no shipped config with _meta.id ${id}`);
    return JSON.parse(readFileSync(join(PRESETS_DIR, file), "utf8"))._meta as { version: number };
}

function index() {
    return parsePreviewIndex(readFileSync(join(PREVIEW_DIR, "previews.json"), "utf8"));
}

describe("parsePreviewIndex", () => {
    it("rejects an envelope it does not implement", () => {
        const doc = JSON.stringify({ ...index(), schema_version: PREVIEW_SCHEMA_VERSION + 1 });
        expect(() => parsePreviewIndex(doc)).toThrow(/schema_version/);
    });

    it("rejects a bbox that is not four microdegree integers with min < max", () => {
        const good = index();
        const inverted = { ...good, bbox: { ...good.bbox, min_lon: good.bbox.max_lon + 1 } };
        expect(() => parsePreviewIndex(JSON.stringify(inverted))).toThrow(/bbox/);
        const fractional = { ...good, bbox: { ...good.bbox, min_lat: 46.7 } };
        expect(() => parsePreviewIndex(JSON.stringify(fractional))).toThrow(/bbox/);
    });

    it("rejects a map entry with no file to fetch", () => {
        const good = index();
        const broken = { ...good, maps: [{ preset_id: "default" }] };
        expect(() => parsePreviewIndex(JSON.stringify(broken))).toThrow(/preset_id and a file/);
    });
});

describe("the committed preview index", () => {
    const doc = index();
    const presets = presetIds();

    it("covers every preset, with the file it names on disk", () => {
        expect(doc.maps.map((m) => m.preset_id).sort()).toEqual([...presets].sort());
        for (const m of doc.maps) {
            expect(existsSync(join(PREVIEW_DIR, m.file)), `${m.file} is missing — re-run builder/bake-previews.sh`).toBe(
                true,
            );
        }
    });

    it("was baked from the preset versions in the tree", () => {
        // A demo map baked from an older revision of a preset shows the wrong styling, and unlike
        // the catalog's lagging artifacts (OBCC §3) there is no reason to tolerate it: re-baking
        // is one local command, not a matrix of regions.
        for (const m of doc.maps) {
            const meta = presetMeta(m.preset_id);
            expect(m.preset_version, `${m.preset_id}'s demo map is stale — re-run builder/bake-previews.sh`).toBe(
                meta.version,
            );
        }
    });

    it("frames a box shaped like the panel", () => {
        // Not a style rule. The camera fits the box on *both* axes, so a box wider than 3:4 would
        // be shown zoomed out to make room — every card at a scale nobody chose. Matching the
        // panel is what makes the pinned ≈4.9 m/px the scale a card actually opens on. The
        // projection's cos(lat) correction is part of the shape, hence the term.
        const b = doc.bbox;
        const midLat = ((b.min_lat + b.max_lat) / 2 / 1e6) * (Math.PI / 180);
        const aspect = ((b.max_lon - b.min_lon) * Math.cos(midLat)) / (b.max_lat - b.min_lat);
        expect(aspect).toBeGreaterThan(0.7);
        expect(aspect).toBeLessThanOrEqual(240 / 320);
    });
});

describe("preset copy", () => {
    it("names presets that exist", () => {
        const presets = presetIds();
        for (const id of presetsWithCopy()) expect(presets).toContain(id);
    });

    it("falls back to the preset's own description, so every card says something", () => {
        expect(presetTagline("no-such-preset", "What the style draws.")).toBe("What the style draws.");
        expect(presetTagline("bikepacking", "What the style draws.")).not.toBe("What the style draws.");
    });
});
