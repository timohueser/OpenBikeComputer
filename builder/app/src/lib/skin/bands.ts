import preset from "../../../../presets/schema.json";
import type { SchemaEntry, SkinStyle } from "../catalog/manifest";

// The packer assigns IDs in feature document order. Catalogs do not yet carry canonical z values.
const canonical = Object.entries(preset.features)
    .flatMap(([tag, values]) => Object.entries(values).map(([value, style]) => ({
        feature_type: `${tag}.${value}`,
        z_index: style.z_index,
    })))
    .map((style, index) => ({ ...style, id: index + 1 }));

/** Refuse unknown assignments and drawing-order changes across the reserved rain band. */
export function skinBandError(
    schema: Pick<SchemaEntry, "id" | "styles">,
    styles: readonly Pick<SkinStyle, "feature_type" | "z_index">[],
): string | null {
    if (
        schema.id !== preset._meta.id ||
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
        if (z > 16 && z < 24) return `${style.feature_type}: drawing order 17–23 is reserved for rain.`;
        if ((z >= 24) !== (expected.z_index >= 24)) {
            return `${style.feature_type}: drawing order must stay ${expected.z_index >= 24 ? "at least 24" : "at most 16"}.`;
        }
    }
    return null;
}
