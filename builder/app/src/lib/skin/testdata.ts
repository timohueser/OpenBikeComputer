// Canonical assignment inputs for skin admission tests; expected validation decisions stay in the tests.
import preset from "../../../../presets/schema.json";
import type { SchemaEntry, SkinEntry, SkinStyle } from "../catalog/manifest";
import { exampleCatalog } from "../catalog/testdata";

const styles: SkinStyle[] = Object.entries(preset.features).flatMap(([tag, values]) =>
    Object.entries(values).map(([value, style]) => ({
        feature_type: `${tag}.${value}`,
        color: 0xffff,
        weight: 1,
        z_index: style.z_index,
        priority: 2,
        line_style: "solid",
        fixed_width: tag === "contour",
        terrain_layer: tag === "contour",
        color2: null,
    })));

export const canonicalSchema: SchemaEntry = {
    ...exampleCatalog.schema,
    id: "bikepacking",
    revision: 1,
    styles: styles.map((style, index) => ({ id: index + 1, feature_type: style.feature_type })),
};

export const canonicalSkin: SkinEntry = {
    id: "default", name: "Default", description: "Day", version: 7,
    marker_color: 0xf800, styles, preview: null,
};

export const canonicalCatalogBody = JSON.stringify({
    ...exampleCatalog, schema: canonicalSchema, skins: [{ ...canonicalSkin, preview: undefined }],
});
