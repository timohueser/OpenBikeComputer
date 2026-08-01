// Browser-local custom skins for the product builder (#1045).
//
// This store deliberately persists only presentation bytes. The schema id and
// revision wrap the records, and every load revalidates the exact feature-type
// set against the current catalog before a custom skin may reach the assembler.
// An old skin is therefore ignored after a schema revision instead of silently
// restyling the wrong ids.

import type { SchemaEntry, SkinEntry, SkinStyle } from "../catalog/manifest";

export const CUSTOM_SKINS_KEY = "obcm.customSkins.v1";
const FORMAT = 1;
const MAX_CUSTOM_SKINS = 24;
const CUSTOM_ID = /^custom-[a-z0-9]+(?:-[a-z0-9]+)*$/;

export interface SkinStorage {
    getItem(key: string): string | null;
    setItem(key: string, value: string): void;
}

export interface CustomSkinRecord {
    skin: SkinEntry;
    /** The hosted/custom skin the editor opened from. Presentation metadata;
     *  the assembler sees only `skin`. */
    based_on: string;
}

interface Envelope {
    format: number;
    schema_id: string;
    schema_revision: number;
    skins: CustomSkinRecord[];
}

export function browserSkinStorage(): SkinStorage | null {
    try {
        return globalThis.localStorage ?? null;
    } catch {
        return null;
    }
}

export function cloneSkin(skin: SkinEntry): SkinEntry {
    return {
        ...skin,
        styles: skin.styles.map((style) => ({ ...style })),
        preview: null,
    };
}

function object(value: unknown): Record<string, unknown> | null {
    return value !== null && typeof value === "object" && !Array.isArray(value)
        ? (value as Record<string, unknown>)
        : null;
}

function integer(value: unknown, min: number, max: number): number | null {
    return typeof value === "number" && Number.isInteger(value) && value >= min && value <= max
        ? value
        : null;
}

function styleFrom(raw: unknown, featureType: string): SkinStyle | null {
    const value = object(raw);
    if (!value || value.feature_type !== featureType || typeof value.dashed !== "boolean") return null;
    const color = integer(value.color, 0, 0xffff);
    const weight = integer(value.weight, 0, 0xff);
    const zIndex = integer(value.z_index, -128, 127);
    const priority = integer(value.priority, 1, 4);
    const color2 = value.color2 === null ? null : integer(value.color2, 0, 0xffff);
    if (
        color === null ||
        weight === null ||
        zIndex === null ||
        priority === null ||
        (color2 === null && value.color2 !== null)
    ) {
        return null;
    }
    return {
        feature_type: featureType,
        color,
        weight,
        z_index: zIndex,
        priority,
        dashed: value.dashed,
        color2,
    };
}

/** Strictly admit a persisted custom skin into the current schema. */
export function validateCustomSkin(raw: unknown, schema: SchemaEntry): SkinEntry | null {
    const value = object(raw);
    if (!value || typeof value.id !== "string" || !CUSTOM_ID.test(value.id)) return null;
    if (typeof value.name !== "string" || !value.name.trim()) return null;
    const version = integer(value.version, 1, Number.MAX_SAFE_INTEGER);
    const markerColor = integer(value.marker_color, 0, 0xffff);
    if (version === null || markerColor === null || !Array.isArray(value.styles)) return null;
    if (value.styles.length !== schema.styles.length) return null;

    const styles: SkinStyle[] = [];
    for (let index = 0; index < schema.styles.length; index++) {
        // Persist in schema order. This makes the JSON handed to the assembler
        // deterministic and structurally prevents feature add/remove/reorder.
        const style = styleFrom(value.styles[index], schema.styles[index].feature_type);
        if (!style) return null;
        styles.push(style);
    }
    return {
        id: value.id,
        name: value.name.trim().slice(0, 64),
        description:
            typeof value.description === "string" && value.description.trim()
                ? value.description.trim().slice(0, 240)
                : "Saved in this browser.",
        version,
        marker_color: markerColor,
        styles,
        preview: null,
    };
}

export function loadCustomSkins(storage: SkinStorage | null, schema: SchemaEntry): CustomSkinRecord[] {
    if (!storage) return [];
    try {
        const text = storage.getItem(CUSTOM_SKINS_KEY);
        if (!text) return [];
        const envelope = object(JSON.parse(text));
        if (
            !envelope ||
            envelope.format !== FORMAT ||
            envelope.schema_id !== schema.id ||
            envelope.schema_revision !== schema.revision ||
            !Array.isArray(envelope.skins)
        ) {
            return [];
        }
        const records: CustomSkinRecord[] = [];
        const ids = new Set<string>();
        for (const raw of envelope.skins.slice(0, MAX_CUSTOM_SKINS)) {
            const record = object(raw);
            const skin = record ? validateCustomSkin(record.skin, schema) : null;
            if (!skin || ids.has(skin.id) || typeof record?.based_on !== "string") continue;
            ids.add(skin.id);
            records.push({ skin, based_on: record.based_on });
        }
        return records;
    } catch {
        // Storage is an optional convenience. A corrupt/denied entry must not
        // stop the catalog or the two hosted skins from loading.
        return [];
    }
}

export function persistCustomSkins(
    storage: SkinStorage | null,
    schema: SchemaEntry,
    records: readonly CustomSkinRecord[],
): void {
    if (!storage) throw new Error("Browser storage is unavailable; this skin cannot be saved here.");
    if (records.length > MAX_CUSTOM_SKINS) {
        throw new Error(`This browser already has ${MAX_CUSTOM_SKINS} custom skins. Delete one before saving another.`);
    }
    const envelope: Envelope = {
        format: FORMAT,
        schema_id: schema.id,
        schema_revision: schema.revision,
        skins: records.map((record) => ({
            skin: cloneSkin(record.skin),
            based_on: record.based_on,
        })),
    };
    try {
        storage.setItem(CUSTOM_SKINS_KEY, JSON.stringify(envelope));
    } catch {
        throw new Error("The browser could not save this skin. Check its site-storage permission and free space.");
    }
}

export function isCustomSkinId(id: string): boolean {
    return CUSTOM_ID.test(id);
}

function freshId(): string {
    const uuid = globalThis.crypto?.randomUUID?.().toLowerCase();
    if (uuid) return `custom-${uuid}`;
    return `custom-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

/** Turn the editor's draft into an assembly-ready, schema-ordered custom skin. */
export function prepareCustomSkin(
    draft: SkinEntry,
    schema: SchemaEntry,
    name: string,
    existing: SkinEntry | null,
    idFactory: () => string = freshId,
): SkinEntry {
    const id = existing && isCustomSkinId(existing.id) ? existing.id : idFactory();
    const candidate: SkinEntry = {
        ...cloneSkin(draft),
        id,
        name: name.trim(),
        description: "Saved in this browser.",
        version: existing && existing.id === id ? existing.version + 1 : 1,
        preview: null,
    };
    const valid = validateCustomSkin(candidate, schema);
    if (!valid) throw new Error("The skin is incomplete or contains a value outside the device format.");
    return valid;
}
