import { describe, expect, it } from "vitest";

import type { SchemaEntry, SkinEntry } from "../catalog/manifest";
import {
    CUSTOM_SKINS_KEY,
    loadCustomSkins,
    persistCustomSkins,
    prepareCustomSkin,
    type SkinStorage,
} from "./custom";

const schema = {
    id: "bikepacking",
    revision: 4,
    styles: [
        { id: 1, feature_type: "highway.primary" },
        { id: 2, feature_type: "natural.water" },
    ],
} as SchemaEntry;

const hosted: SkinEntry = {
    id: "default",
    name: "Default",
    description: "Day",
    version: 7,
    marker_color: 0xf800,
    styles: [
        { feature_type: "highway.primary", color: 0xffff, weight: 3, z_index: 5, priority: 2, dashed: false, color2: null },
        { feature_type: "natural.water", color: 0x001f, weight: 1, z_index: 1, priority: 3, dashed: false, color2: null },
    ],
    preview: null,
};

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
        expect(loadCustomSkins(storage, { ...schema, revision: 5 })).toEqual([]);
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
