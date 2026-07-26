// The consumer half of OBCC §7: a manifest is wholly valid or wholly rejected.
// Every case below is a document that must be refused *in full* — never partly
// read, never read past the field that was wrong.
//
// The first test is the one that matters most: the manifest the real generator
// writes must parse. It reads the checked-in example rather than a hand-written
// fixture, so a producer-side change to the shape fails here instead of in a
// browser.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { CatalogFormatError, parseCatalog } from "./manifest";

const EXAMPLE_PATH = new URL(
    "../../../../../../firmware/obc-pack/schema/catalog.example.json",
    import.meta.url,
);
const EXAMPLE = readFileSync(EXAMPLE_PATH, "utf8");

/** The example parsed loose. These cases deliberately break its shape, and
 *  typing each mutation precisely would only restate the parser's own job. */
type LooseDoc = any; // deliberately loose: these cases break the shape on purpose

/** The example, with one edit applied to the parsed tree. */
function mutated(edit: (doc: LooseDoc) => void): string {
    const doc = JSON.parse(EXAMPLE) as LooseDoc;
    edit(doc);
    return JSON.stringify(doc);
}

describe("parseCatalog", () => {
    it("accepts the manifest obc-pack actually writes", () => {
        const catalog = parseCatalog(EXAMPLE);
        expect(catalog.schema_version).toBe(1);
        expect(catalog.presets.map((p) => p.id)).toEqual(["default", "minimal"]);
        expect(catalog.artifacts).toHaveLength(5);
        const swiss = catalog.artifacts.find(
            (a) => a.region_id === "europe/switzerland" && a.preset_id === "default",
        )!;
        expect(swiss.obcm_version).toBe(10);
        expect(swiss.sha256).toMatch(/^[0-9a-f]{64}$/);
        expect(swiss.bbox.min_lat).toBe(45817995);
    });

    it("ignores fields it does not recognise", () => {
        // §1: a new OPTIONAL field is not a breaking change.
        const doc = mutated((d) => {
            d.mirrors = ["https://mirror.example.org/"];
            d.artifacts[0].torrent = "magnet:?xt=…";
        });
        expect(parseCatalog(doc).artifacts).toHaveLength(5);
    });

    it.each<[string, (d: LooseDoc) => void]>([
        ["an envelope version it does not implement", (d) => (d.schema_version = 2)],
        ["a missing schema_version", (d) => delete d.schema_version],
        ["a missing artifacts array", (d) => delete d.artifacts],
        ["a malformed region id", (d) => (d.artifacts[0].region_id = "Europe/Monaco")],
        ["a truncated sha256", (d) => (d.artifacts[0].sha256 = "abc123")],
        ["a missing size", (d) => delete d.artifacts[0].bytes],
        ["a fractional size", (d) => (d.artifacts[0].bytes = 1.5)],
        ["a timestamp in another spelling", (d) => (d.artifacts[0].built_at = "2026-07-20T02:44:19.5Z")],
        ["a date that does not exist", (d) => (d.artifacts[0].source_snapshot = "2026-02-30")],
        ["a bbox whose min exceeds its max", (d) => (d.artifacts[0].bbox.min_lat = 89_000_000)],
        ["a latitude out of range", (d) => (d.artifacts[0].bbox.max_lat = 91_000_000)],
        ["an artifact naming a preset that isn't listed", (d) => (d.artifacts[0].preset_id = "vivid")],
        // §3: an artifact ahead of the preset config it claims to come from.
        ["an artifact whose preset_version is ahead of the preset", (d) => (d.artifacts[0].preset_version = 9)],
        [
            "two artifacts for one (region, preset)",
            (d) => d.artifacts.push(JSON.parse(JSON.stringify(d.artifacts[0]))),
        ],
    ])("rejects %s", (_what, edit) => {
        expect(() => parseCatalog(mutated(edit))).toThrow(CatalogFormatError);
    });

    it("rejects a truncated body rather than reading its prefix", () => {
        // The property §7 leans on: JSON is self-delimiting, so no proper
        // prefix of a valid document parses.
        const half = EXAMPLE.slice(0, Math.floor(EXAMPLE.length / 2));
        expect(() => parseCatalog(half)).toThrow(CatalogFormatError);
    });

    it("accepts an artifact whose styling lags the preset", () => {
        // §3: lower is a partial re-bake and MUST NOT be refused.
        const doc = mutated((d) => (d.artifacts[0].preset_version = 1));
        expect(parseCatalog(doc).artifacts[0].preset_version).toBe(1);
    });

    it("treats an explicitly null preview as no preview", () => {
        const doc = mutated((d) => (d.presets[0].preview = null));
        expect(parseCatalog(doc).presets[0].preview).toBeUndefined();
    });
});
