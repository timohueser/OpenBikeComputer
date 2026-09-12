import { describe, expect, it } from "vitest";

import { canonicalSchema as schema, canonicalSkin as hosted } from "./testdata";
import { cloneSkin } from "./custom";
import {
    CUSTOM_SKINS_KEY,
    loadCustomSkins,
    persistCustomSkins,
    prepareCustomSkin,
    type SkinStorage,
} from "./custom";

class MemoryStorage implements SkinStorage {
    readonly values = new Map<string, string>();
    getItem(key: string): string | null {
        return this.values.get(key) ?? null;
    }
    setItem(key: string, value: string): void {
        this.values.set(key, value);
    }
}

describe("custom skin storage", () => {
    it("saves a hosted skin as a schema-ordered custom skin and reloads it", () => {
        const storage = new MemoryStorage();
        const skin = prepareCustomSkin(hosted, schema, "Morning roads", null, () => "custom-morning-roads");
        expect(skin).toMatchObject({ id: "custom-morning-roads", name: "Morning roads", version: 1 });
        persistCustomSkins(storage, schema, [{ skin, based_on: "default" }]);
        expect(loadCustomSkins(storage, schema)).toEqual([{ skin, based_on: "default" }]);
    });

    it("increments an edited custom skin without changing its identity", () => {
        const first = prepareCustomSkin(hosted, schema, "Mine", null, () => "custom-mine");
        const second = prepareCustomSkin({ ...first, marker_color: 0x07e0 }, schema, "Mine v2", first);
        expect(second).toMatchObject({ id: "custom-mine", version: 2, marker_color: 0x07e0 });
    });

    it("ignores stale, reordered, or schema-space records", () => {
        const storage = new MemoryStorage();
        const skin = prepareCustomSkin(hosted, schema, "Mine", null, () => "custom-mine");
        persistCustomSkins(storage, schema, [{ skin, based_on: "default" }]);

        const raw = JSON.parse(storage.values.get(CUSTOM_SKINS_KEY)!);
        raw.skins[0].skin.styles.reverse();
        storage.values.set(CUSTOM_SKINS_KEY, JSON.stringify(raw));
        expect(loadCustomSkins(storage, schema)).toEqual([]);

        persistCustomSkins(storage, schema, [{ skin, based_on: "default" }]);
        expect(loadCustomSkins(storage, { ...schema, revision: schema.revision + 1 })).toEqual([]);
    });

    it("does not let a storage failure masquerade as a saved skin", () => {
        const denied: SkinStorage = {
            getItem: () => null,
            setItem: () => {
                throw new Error("denied");
            },
        };
        expect(() => persistCustomSkins(denied, schema, [])).toThrow(/could not save/);
    });

    it("refuses to show a twenty-fifth skin that would disappear on refresh", () => {
        const storage = new MemoryStorage();
        const records = Array.from({ length: 25 }, (_, index) => ({
            skin: prepareCustomSkin(hosted, schema, `Skin ${index}`, null, () => `custom-skin-${index}`),
            based_on: "default",
        }));
        expect(() => persistCustomSkins(storage, schema, records)).toThrow(/24 custom skins/);
        expect(storage.values.has(CUSTOM_SKINS_KEY)).toBe(false);
    });
});


describe("custom skin rain bands", () => {
    it.each([
        ["highway.primary", 16], ["natural.water", 24],
        ["highway.primary", 17], ["natural.water", 23],
    ])("refuses %s at z=%i on prepare, persist and reload", (feature, z) => {
        const valid = prepareCustomSkin(hosted, schema, "Mine", null, () => "custom-mine");
        const storage = new MemoryStorage();
        persistCustomSkins(storage, schema, [{ skin: valid, based_on: "default" }]);
        const saved = storage.values.get(CUSTOM_SKINS_KEY)!;
        const invalid = cloneSkin(valid);
        invalid.styles.find((style) => style.feature_type === feature)!.z_index = z;
        expect(() => prepareCustomSkin(invalid, schema, "Mine", valid)).toThrow(/drawing order/);
        expect(() => persistCustomSkins(storage, schema, [{ skin: invalid, based_on: "default" }])).toThrow(/drawing order/);
        expect(storage.values.get(CUSTOM_SKINS_KEY), "failed admission must not replace saved bytes").toBe(saved);
        const envelope = JSON.parse(saved);
        envelope.skins[0].skin = invalid;
        storage.values.set(CUSTOM_SKINS_KEY, JSON.stringify(envelope));
        expect(loadCustomSkins(storage, schema)).toEqual([]);
    });

    it("allows each band's endpoints and reorders styles within a band", () => {
        const draft = cloneSkin(hosted);
        draft.styles.find((style) => style.feature_type === "highway.primary")!.z_index = 24;
        draft.styles.find((style) => style.feature_type === "highway.track")!.z_index = 127;
        draft.styles.find((style) => style.feature_type === "natural.water")!.z_index = -128;
        draft.styles.find((style) => style.feature_type === "natural.land")!.z_index = 16;
        const skin = prepareCustomSkin(draft, schema, "Mine", null, () => "custom-mine");
        const storage = new MemoryStorage();
        persistCustomSkins(storage, schema, [{ skin, based_on: "default" }]);
        expect(loadCustomSkins(storage, schema)[0].skin.styles).toEqual(draft.styles);
    });

    it("refuses unknown feature, ID, and schema assignments without admitting saved records", () => {
        const skin = prepareCustomSkin(hosted, schema, "Mine", null, () => "custom-mine");
        const storage = new MemoryStorage();
        persistCustomSkins(storage, schema, [{ skin, based_on: "default" }]);
        for (const unknown of [
            { ...schema, id: "another-schema" },
            { ...schema, revision: 2 },
            { ...schema, styles: schema.styles.slice(1) },
            { ...schema, styles: schema.styles.map((style, index) => index === 0 ? { ...style, id: 99 } : style) },
            { ...schema, styles: schema.styles.map((style, index) => index === 0 ? { ...style, feature_type: "unknown.layer" } : style) },
        ]) {
            expect(() => prepareCustomSkin(hosted, unknown, "Mine", null)).toThrow(/unavailable for this map schema/);
            expect(() => persistCustomSkins(storage, unknown, [{ skin, based_on: "default" }])).toThrow(/unavailable for this map schema/);
            // Match the envelope to the new catalog so its existing identity check cannot mask admission.
            const envelope = JSON.parse(storage.values.get(CUSTOM_SKINS_KEY)!);
            envelope.schema_id = unknown.id;
            envelope.schema_revision = unknown.revision;
            storage.values.set(CUSTOM_SKINS_KEY, JSON.stringify(envelope));
            expect(loadCustomSkins(storage, unknown)).toEqual([]);
        }
    });
});
