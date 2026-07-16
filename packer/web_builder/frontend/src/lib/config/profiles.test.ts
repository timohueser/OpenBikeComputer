import { describe, expect, it } from "vitest";
import type { NavProfile, PackConfig, SchemaEnvelope } from "./model";
import {
    addProfile,
    cellValue,
    checkMultiplier,
    clearCell,
    displayProfiles,
    ensureRouting,
    isExplicit,
    profileDefault,
    readProfileSchema,
    removeProfile,
    resetProfile,
    setCell,
} from "./profiles";

// A schema envelope shaped like `obc-pack schema`'s routing bits (trimmed).
const road: NavProfile = {
    name: "Road",
    default: 3.0,
    highway: { cycleway: 1.0, primary: 1.8, steps: "forbidden" },
    surface: { paved: 1.0, gravel: 5.0 },
};
const gravel: NavProfile = { name: "Gravel", default: 2.0, highway: { track: 1.2 } };

function schemaEnvelope(): SchemaEnvelope {
    return {
        schema_version: 1,
        format_version: 9,
        source: "binary",
        schema: {
            $defs: {
                style: { properties: {} },
                multiplier: { oneOf: [{ type: "number", minimum: 1.0 }, { const: "forbidden" }] },
                profile: {
                    properties: {
                        name: { maxLength: 12, "x-maxUtf8Bytes": 12 },
                        default: { default: 2.0 },
                        highway: {
                            propertyNames: {
                                enum: ["cycleway", "path", "track", "steps", "primary"],
                            },
                        },
                        surface: { propertyNames: { enum: ["unknown", "paved", "gravel"] } },
                    },
                },
            },
            properties: {
                routing: {
                    properties: { profiles: { minItems: 1, maxItems: 8 } },
                    default: { profiles: [road, gravel] },
                },
            },
        },
    } as unknown as SchemaEnvelope;
}

const PS = readProfileSchema(schemaEnvelope())!;

describe("readProfileSchema", () => {
    it("reads class enums, bounds, and shipped defaults from the schema", () => {
        expect(PS.highwayClasses).toEqual(["cycleway", "path", "track", "steps", "primary"]);
        expect(PS.surfaceClasses).toEqual(["unknown", "paved", "gravel"]);
        expect(PS.multiplierMin).toBe(1.0);
        expect(PS.defaultMultiplier).toBe(2.0);
        expect(PS.minProfiles).toBe(1);
        expect(PS.maxProfiles).toBe(8);
        expect(PS.nameMaxBytes).toBe(12);
        expect(PS.defaultProfiles.map((p) => p.name)).toEqual(["Road", "Gravel"]);
    });

    it("returns null when the schema doesn't describe routing", () => {
        const bare = { schema_version: 1, format_version: 6, source: "binary", schema: { $defs: { style: { properties: {} } } } } as unknown as SchemaEnvelope;
        expect(readProfileSchema(bare)).toBeNull();
        expect(readProfileSchema(null)).toBeNull();
    });

    it("prefers the schema's UTF-8 byte cap over JSON Schema character length", () => {
        const env = schemaEnvelope() as any;
        env.schema.$defs.profile.properties.name.maxLength = 99;
        env.schema.$defs.profile.properties.name["x-maxUtf8Bytes"] = 12;
        expect(readProfileSchema(env)?.nameMaxBytes).toBe(12);
    });
});

describe("cell value / explicit / inherit", () => {
    it("shows the explicit override, and the profile default for unlisted classes", () => {
        expect(cellValue(road, "highway", "cycleway", PS)).toBe(1.0);
        expect(isExplicit(road, "highway", "cycleway")).toBe(true);
        // `path` is not listed → inherits the profile default (3.0).
        expect(cellValue(road, "highway", "path", PS)).toBe(3.0);
        expect(isExplicit(road, "highway", "path")).toBe(false);
        // steps is explicitly forbidden.
        expect(cellValue(road, "highway", "steps", PS)).toBe("forbidden");
    });

    it("falls back to the schema default multiplier when the profile omits `default`", () => {
        const p: NavProfile = { name: "X" };
        expect(profileDefault(p, PS)).toBe(2.0);
        expect(cellValue(p, "surface", "gravel", PS)).toBe(2.0);
    });
});

describe("setCell / clearCell", () => {
    it("writes an explicit override and reverts it to inherit", () => {
        const p: NavProfile = { name: "X", default: 2.0 };
        setCell(p, "highway", "primary", 4.0);
        expect(p.highway).toEqual({ primary: 4.0 });
        expect(cellValue(p, "highway", "primary", PS)).toBe(4.0);
        setCell(p, "surface", "paved", "forbidden");
        expect(p.surface).toEqual({ paved: "forbidden" });
        clearCell(p, "highway", "primary");
        expect(isExplicit(p, "highway", "primary")).toBe(false);
        expect(cellValue(p, "highway", "primary", PS)).toBe(2.0);
    });
});

describe("multiplier validation", () => {
    it("rejects a sub-minimum value with an admissibility hint", () => {
        const r = checkMultiplier(0.5, PS.multiplierMin);
        expect(r.ok).toBe(false);
        expect(r.hint).toMatch(/admissible/);
        expect(r.hint).toMatch(/shortest-path/);
    });

    it("rejects a non-number (cleared field) without the scary message", () => {
        const r = checkMultiplier(NaN, PS.multiplierMin);
        expect(r.ok).toBe(false);
        expect(r.hint).toMatch(/number/);
    });

    it("accepts a value at or above the minimum", () => {
        expect(checkMultiplier(1.0, PS.multiplierMin).ok).toBe(true);
        expect(checkMultiplier(5.5, PS.multiplierMin).ok).toBe(true);
    });
});

describe("routing lifecycle over a config", () => {
    function bareConfig(): PackConfig {
        return { lods: [{ max_mpp: null, simplify: 0 }], features: {}, marker: { color: "0xF800" } };
    }

    it("displays shipped defaults for a config with no routing section", () => {
        const cfg = bareConfig();
        expect(cfg.routing).toBeUndefined();
        expect(displayProfiles(cfg, PS).map((p) => p.name)).toEqual(["Road", "Gravel"]);
    });

    it("materializes routing from the shipped defaults on first ensure", () => {
        const cfg = bareConfig();
        const routing = ensureRouting(cfg, PS);
        expect(cfg.routing).toBe(routing);
        expect(routing.profiles.map((p) => p.name)).toEqual(["Road", "Gravel"]);
        // A deep copy — editing the config must not mutate the schema defaults.
        routing.profiles[0].name = "Edited";
        expect(PS.defaultProfiles[0].name).toBe("Road");
    });

    it("adds profiles up to the schema max, seeding them at the default", () => {
        const cfg = bareConfig();
        ensureRouting(cfg, PS); // 2 shipped defaults
        for (let i = 2; i < 8; i++) expect(addProfile(cfg, PS)).not.toBeNull();
        expect(cfg.routing!.profiles.length).toBe(8);
        expect(addProfile(cfg, PS)).toBeNull(); // capped at maxProfiles
        const added = cfg.routing!.profiles[7];
        expect(added.default).toBe(2.0);
        expect(added.highway).toBeUndefined(); // every class inherits the default
    });

    it("refuses to remove the last profile", () => {
        const cfg = bareConfig();
        ensureRouting(cfg, PS).profiles = [{ name: "Only", default: 2.0 }];
        expect(removeProfile(cfg, 0, PS)).toBe(false);
        expect(cfg.routing!.profiles.length).toBe(1);
    });

    it("resets a profile to its shipped default by name", () => {
        const cfg = bareConfig();
        ensureRouting(cfg, PS);
        cfg.routing!.profiles[0].highway = { cycleway: 9.0 }; // tamper with Road
        resetProfile(cfg, 0, PS);
        expect(cfg.routing!.profiles[0].highway).toEqual(road.highway);
        expect(cfg.routing!.profiles[0].highway!.cycleway).toBe(1.0);
    });
});
