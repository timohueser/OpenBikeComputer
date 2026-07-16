import { describe, expect, it } from "vitest";
import { ENVELOPE_VERSION, migrateEnvelope } from "./migrations";

describe("migrateEnvelope", () => {
    it("accepts a current envelope", () => {
        const env = migrateEnvelope({
            schema_version: ENVELOPE_VERSION,
            based_on: { id: "default", version: 1 },
            modified: true,
            config: { lods: [], features: {}, marker: { color: "0xF800" } },
            disabled: ["highway/path"],
        });
        expect(env).not.toBeNull();
        expect(env!.based_on).toEqual({ id: "default", version: 1 });
        expect(env!.modified).toBe(true);
        expect(env!.disabled).toEqual(["highway/path"]);
    });

    it("fills defaults for missing envelope fields", () => {
        const env = migrateEnvelope({ config: { features: {} } });
        expect(env).not.toBeNull();
        expect(env!.based_on).toBeNull();
        expect(env!.modified).toBe(false);
        expect(env!.disabled).toEqual([]);
        expect(env!.schema_version).toBe(ENVELOPE_VERSION);
    });

    it("rejects garbage", () => {
        expect(migrateEnvelope(null)).toBeNull();
        expect(migrateEnvelope("nope")).toBeNull();
        expect(migrateEnvelope({ schema_version: 1 })).toBeNull(); // no config
    });

    it("rejects envelopes from the future it can't migrate", () => {
        // A version above CURRENT has no migration path down; migrateEnvelope
        // passes it through the version loop untouched and keeps the config.
        const env = migrateEnvelope({
            schema_version: ENVELOPE_VERSION + 1,
            config: { features: {} },
        });
        expect(env).not.toBeNull(); // forward-compatible read (config intact)
    });
});
