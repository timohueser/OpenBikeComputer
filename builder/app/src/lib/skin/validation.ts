import preset from "../../../../presets/schema.json";
import type { SchemaEntry, SkinStyle } from "../catalog/manifest";

// Catalog content revision for these assignments; this is not the preset metadata version.
const CATALOG_SCHEMA_REVISION = 1;

// The packer assigns IDs in feature document order.
const canonical = Object.entries(preset.features)
    .flatMap(([tag, values]) => Object.entries(values).map(([value]) => ({
        feature_type: `${tag}.${value}`,
    })))
    .map((style, index) => ({ ...style, id: index + 1 }));

/** Check schema assignments and the drawing-order field range. */
export function skinStyleError(
    schema: Pick<SchemaEntry, "id" | "revision" | "styles">,
    styles: readonly Pick<SkinStyle, "feature_type" | "z_index">[],
): string | null {
    if (
        schema.id !== preset._meta.id ||
        schema.revision !== CATALOG_SCHEMA_REVISION ||
        schema.styles.length !== canonical.length ||
        schema.styles.some((style, index) =>
            style.id !== canonical[index].id || style.feature_type !== canonical[index].feature_type)
    ) {
        return "Custom skin drawing order is unavailable for this map schema.";
    }
    if (styles.length !== canonical.length) return "The skin must cover every map style in schema order.";
    for (const [index, style] of styles.entries()) {
        const expected = canonical[index];
        if (style.feature_type !== expected.feature_type) return "The skin must follow the map's style assignment.";
        const z = style.z_index;
        if (!Number.isInteger(z) || z < -128 || z > 127) return "Drawing order must be an integer from -128 to 127.";
    }
    return null;
}
