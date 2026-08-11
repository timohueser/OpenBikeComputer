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

// Mirrors the OBCM style schema obc-pack serves (#557): the five original
// fields plus line_style + color2. Values are placeholders — buildConfigForSubmit
// only reads the property *names* (its known-key set).
const mockSchema: SchemaEnvelope = {
    schema_version: 1,
    format_version: 10,
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
                    line_style: { enum: ["solid", "dashed"], default: "solid" },
                    color2: { $ref: "#/$defs/color" },
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
        config.features.highway.motorway.experimental = true; // not in the schema
        const out = buildConfigForSubmit(config, [], mockSchema);
        expect(out.config.features.highway.motorway.experimental).toBeUndefined();
        expect(out.strippedKeys).toEqual(["experimental"]);
    });

    it("keeps line_style + color2 now that the v10 schema declares them", () => {
        const { config } = normalizeConfig(sampleConfig);
        config.features.highway.motorway.line_style = "dashed";
        config.features.highway.motorway.color2 = "0x8410";
        const out = buildConfigForSubmit(config, [], mockSchema);
        expect(out.config.features.highway.motorway.line_style).toBe("dashed");
        expect(out.config.features.highway.motorway.color2).toBe("0x8410");
        expect(out.strippedKeys).toEqual([]);
    });

    it("round-trips an absent color2 as absent (no key, no null)", () => {
        const { config } = normalizeConfig(sampleConfig);
        const style = buildConfigForSubmit(config, [], mockSchema).config.features.highway.motorway;
        expect("color2" in style).toBe(false);
        expect(JSON.stringify(style).includes("color2")).toBe(false);
    });

    it("round-trips a present color2 (including black 0x0000, a legit color)", () => {
        const { config } = normalizeConfig(sampleConfig);
        config.features.highway.motorway.color2 = "0x0000"; // black is NOT "unset"
        const style = buildConfigForSubmit(config, [], mockSchema).config.features.highway.motorway;
        expect("color2" in style).toBe(true);
        expect(style.color2).toBe("0x0000");
    });

    it("clearing color2 (undefined) drops the key from the submitted config", () => {
        const { config } = normalizeConfig(sampleConfig);
        // The StyleTable clear handler `delete`s the key; a stray present-but-
        // undefined value must still emit as absent, not as `"color2":null`.
        config.features.highway.motorway.color2 = undefined;
        const out = buildConfigForSubmit(config, [], mockSchema);
        const style = out.config.features.highway.motorway;
        expect("color2" in style).toBe(false);
        expect(out.strippedKeys).toEqual([]); // dropping undefined isn't "stripping"
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

    it("emits a positive min_area_px but drops it on the finest tier", () => {
        const cfg = normalizeConfig({
            lods: [
                { max_mpp: null, simplify: 120, min_area_px: 6 },
                { max_mpp: 120, simplify: 18, min_area_px: 4 },
                { max_mpp: 18, simplify: 0, min_area_px: 4 }, // finest — packer ignores it
            ],
            features: {},
            marker: { color: "0xF800" },
        }).config;
        const out = buildConfigForSubmit(cfg, [], mockSchema).config;
        expect(out.lods[0].min_area_px).toBe(6);
        expect(out.lods[1].min_area_px).toBe(4);
        expect("min_area_px" in out.lods[2]).toBe(false);
    });

    it("round-trips coverage_simplify, and omits it where it is off", () => {
        // The shipped preset turns the pass on for its two coarsest tiers; a config the builder
        // loads and re-submits has to hand the packer back the same flag, or a rebuild silently
        // loses the shared-boundary simplify (and with it the elimination those tiers need).
        const cfg = normalizeConfig({
            lods: [
                { max_mpp: null, simplify: 2200, min_area_px: 250, coverage_simplify: true },
                { max_mpp: 400, simplify: 700, min_area_px: 700, coverage_simplify: true },
                { max_mpp: 120, simplify: 200, min_area_px: 50 },
                { max_mpp: 30, simplify: 0 },
            ],
            features: {},
            marker: { color: "0xF800" },
        }).config;
        expect(cfg.lods.map((l) => l.coverage_simplify)).toEqual([true, true, undefined, undefined]);
        const out = buildConfigForSubmit(cfg, [], mockSchema).config;
        expect(out.lods[0].coverage_simplify).toBe(true);
        expect(out.lods[1].coverage_simplify).toBe(true);
        expect("coverage_simplify" in out.lods[2]).toBe(false);
        expect("coverage_simplify" in out.lods[3]).toBe(false);
    });

    it("round-trips min_line_km, and omits it where it is off", () => {
        // Same hazard as coverage_simplify, and worse to lose quietly: the shipped preset's two
        // coarse tiers only fit highway=primary because the stub cull frees the spans for it, so
        // a rebuild that dropped the knob would come back over budget with the roads torn out.
        const cfg = normalizeConfig({
            lods: [
                { max_mpp: null, simplify: 3000, min_area_px: 350, coverage_simplify: true, min_line_km: 1.0 },
                { max_mpp: 400, simplify: 1500, min_area_px: 1000, coverage_simplify: true, min_line_km: 0.5 },
                { max_mpp: 120, simplify: 200, min_area_px: 50 },
                { max_mpp: 30, simplify: 0 },
            ],
            features: {},
            marker: { color: "0xF800" },
        }).config;
        expect(cfg.lods.map((l) => l.min_line_km)).toEqual([1.0, 0.5, undefined, undefined]);
        const out = buildConfigForSubmit(cfg, [], mockSchema).config;
        expect(out.lods[0].min_line_km).toBe(1.0);
        expect(out.lods[1].min_line_km).toBe(0.5);
        expect("min_line_km" in out.lods[2]).toBe(false);
        expect("min_line_km" in out.lods[3]).toBe(false);
    });

    it("omits min_area_px entirely when it is 0/absent (byte-identical off)", () => {
        const cfg = normalizeConfig({
            lods: [{ max_mpp: null, simplify: 0, min_area_px: 0 }, { max_mpp: 30, simplify: 0 }],
            features: {},
            marker: { color: "0xF800" },
        }).config;
        const out = buildConfigForSubmit(cfg, [], mockSchema).config;
        expect("min_area_px" in out.lods[0]).toBe(false);
        expect("min_area_px" in out.lods[1]).toBe(false);
    });

    it("emits merge_fills only when on (absent/false ⇒ byte-identical off)", () => {
        const base = { lods: [{ max_mpp: null, simplify: 0 }], features: {}, marker: { color: "0xF800" } };

        const off = buildConfigForSubmit(normalizeConfig(base).config, [], mockSchema).config;
        expect("merge_fills" in off).toBe(false);

        const explicitOff = buildConfigForSubmit(
            normalizeConfig({ ...base, merge_fills: false }).config,
            [],
            mockSchema,
        ).config;
        expect("merge_fills" in explicitOff).toBe(false);

        const on = buildConfigForSubmit(normalizeConfig({ ...base, merge_fills: true }).config, [], mockSchema).config;
        expect(on.merge_fills).toBe(true);
    });

    it("emits merge_lines only when on (absent/false ⇒ byte-identical off)", () => {
        const base = { lods: [{ max_mpp: null, simplify: 0 }], features: {}, marker: { color: "0xF800" } };

        const off = buildConfigForSubmit(normalizeConfig(base).config, [], mockSchema).config;
        expect("merge_lines" in off).toBe(false);

        const explicitOff = buildConfigForSubmit(
            normalizeConfig({ ...base, merge_lines: false }).config,
            [],
            mockSchema,
        ).config;
        expect("merge_lines" in explicitOff).toBe(false);

        const on = buildConfigForSubmit(normalizeConfig({ ...base, merge_lines: true }).config, [], mockSchema).config;
        expect(on.merge_lines).toBe(true);
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
