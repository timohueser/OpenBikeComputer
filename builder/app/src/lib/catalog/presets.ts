// The manifest's `presets[]` as the picker's preset list. Shared by the host
// that serves the seam (`platform.presets()`) and by the catalog store's cache
// fallback, so a fresh manifest and a cached one produce the same list rather
// than two mappings that can drift.

import type { Preset } from "../config/model";
import type { Catalog } from "./manifest";

/**
 * A catalog preset carries no packer config — there is no packer on the tier
 * that reads a catalog, and the config lives with the bakery that used it. What
 * it does carry is B2's preview, whose reference the host has already resolved
 * to a URL (OBCC §2: resolved against the same base as an artifact's `url`).
 */
export function catalogPresets(catalog: Catalog): Preset[] {
    return catalog.presets.map((p) => ({
        id: p.id,
        name: p.name,
        description: p.description,
        version: p.version,
        ...(p.preview ? { preview: p.preview } : {}),
    }));
}
