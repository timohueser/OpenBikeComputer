import { describe, expect, it } from "vitest";
import { buildConfigForSubmit, normalizeConfig, type SchemaEnvelope } from "./model";

const sampleConfig = {
    _meta: { id: "x", name: "X", version: 1 },
    lods: [
        { max_mpp: 99, simplify: 50 }, // coarsest must be pinned back to null
        { max_mpp: 120, simplify: 12 },
    ],
    features: {
        highway: {
            motorway: { color: "0xFAA0", z_index: 60, weight: 3, min_lod: 0 },
            path: { color: "0xAAA0", min_lod: 9 }, // out of range for 2 lods
        },
        natural: {
            land: { color: "0xFFFF", z_index: 1, min_lod: 0, priority: 1 },
        },
    },
    marker: { color: "0xF800" },
    disabled: ["highway/path"],
};

const mockSchema: SchemaEnvelope = {
    schema_version: 1,
    format_version: 6,
    source: "binary",
    schema: {
        $defs: {
            style: {
                properties: {
                    color: {},
                    z_index: {},
                    weight: {},
                    priority: {},
                    min_lod: {},
                },
            },
        },
    },
};

describe("normalizeConfig", () => {
    it("strips _meta, lifts disabled, pins the coarsest tier, clamps min_lod", () => {
        const { config, disabled } = normalizeConfig(sampleConfig);
        const raw = config as unknown as Record<string, unknown>;
        expect(raw._meta).toBeUndefined();
        expect(disabled).toEqual(["highway/path"]);
        expect(raw.disabled).toBeUndefined();
        expect(config.lods[0].max_mpp).toBeNull();
        expect(config.features.highway.path.min_lod).toBe(1);
    });

    it("defaults an empty config to one coarsest tier and a red marker", () => {
        const { config } = normalizeConfig({});
        expect(config.lods).toEqual([{ max_mpp: null, simplify: 0 }]);
        expect(config.marker).toEqual({ color: "0xF800" });
    });
});

describe("buildConfigForSubmit", () => {
    it("drops disabled features and fills priority", () => {
        const { config } = normalizeConfig(sampleConfig);
        const out = buildConfigForSubmit(config, ["highway/path"], mockSchema);
        expect(out.config.features.highway.path).toBeUndefined();
        expect(out.config.features.highway.motorway.priority).toBe(3);
        expect(out.config.features.natural.land.priority).toBe(1);
    });

    it("strips per-style keys the served schema does not declare", () => {
        const { config } = normalizeConfig(sampleConfig);
        config.features.highway.motorway.line_style = "dashed"; // a v6 field, v5 schema
        const out = buildConfigForSubmit(config, [], mockSchema);
        expect(out.config.features.highway.motorway.line_style).toBeUndefined();
        expect(out.strippedKeys).toEqual(["line_style"]);
    });

    it("keeps v6 fields once the schema declares them", () => {
        const { config } = normalizeConfig(sampleConfig);
        config.features.highway.motorway.line_style = "dashed";
        const v6 = structuredClone(mockSchema);
        v6.schema.$defs.style.properties.line_style = {};
        const out = buildConfigForSubmit(config, [], v6);
        expect(out.config.features.highway.motorway.line_style).toBe("dashed");
        expect(out.strippedKeys).toEqual([]);
    });

    it("carries the routing section through to the submitted config", () => {
        const { config } = normalizeConfig({
            ...sampleConfig,
            routing: {
                min_component_edges: 40,
                profiles: [{ name: "Road", default: 3, highway: { steps: "forbidden" } }],
            },
        });
        const out = buildConfigForSubmit(config, [], mockSchema).config;
        expect(out.routing).toEqual({
            min_component_edges: 40,
            profiles: [{ name: "Road", default: 3, highway: { steps: "forbidden" } }],
        });
        // …and it's a copy, not an alias into the working config.
        out.routing!.profiles[0].name = "Mutated";
        expect(config.routing!.profiles[0].name).toBe("Road");
    });

    it("omits routing entirely when the config has none (CLI-default parity)", () => {
        const { config } = normalizeConfig(sampleConfig);
        const out = buildConfigForSubmit(config, [], mockSchema).config;
        expect("routing" in out).toBe(false);
    });

    it("preserves feature key order (style IDs are document order)", () => {
        // Build a config whose keys would re-sort alphabetically if mishandled.
        const cfg = normalizeConfig({
            lods: [{ max_mpp: null, simplify: 0 }],
            features: {
                waterway: { river: { color: "0x555F" } },
                highway: { primary: { color: "0xFD40" }, cycleway: { color: "0x501F" } },
            },
            marker: { color: "0xF800" },
        }).config;
        const out = buildConfigForSubmit(cfg, [], null).config;
        expect(Object.keys(out.features)).toEqual(["waterway", "highway"]);
        expect(Object.keys(out.features.highway)).toEqual(["primary", "cycleway"]);
        // …and JSON serialization (what the server receives) keeps that order.
        expect(JSON.stringify(out.features).indexOf("waterway")).toBeLessThan(
            JSON.stringify(out.features).indexOf("highway"),
        );
    });
});
