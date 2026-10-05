import { layers, namedFlavor, type Flavor } from "@protomaps/basemaps";
import type { ExpressionSpecification, StyleSpecification, LayerSpecification } from "maplibre-gl";
import { categoryIds, placeCategories, poiKinds, type PlaceCategory } from "./poi-kinds";
import type { BasemapConfig } from "../map/basemap-config";
import type { PlannerConfig } from "./config";

/** The Terrarium terrain tiles: their pixel size and deepest zoom. */
export const DEM_TILE = 512, DEM_MAX_ZOOM = 12;

/** The credit is the release's, from data/sources.toml; Protomaps is the style the basemap is drawn with. */
const basemapSource = (config: BasemapConfig) => ({
    type: "vector",
    url: config.basemap,
    attribution: `${config.attribution} · <a href="https://protomaps.com">Protomaps</a>`,
} as const);

type Tier = "ground" | "zone" | "detail" | "structure";

// Every land use kind of the planner tiles, in the colour a rider reads it by. Open ground is a
// quiet family a shade under the paper, told apart by hue, so the relief above it keeps its
// structure; forest, parks, water and built-up land carry the contrast. Overlaps resolve the same
// way whatever order the tile stores them in: within a tier a later row draws over an earlier
// one (its row index is the fill sort key), zones draw over the ground, details over the zones,
// and structures over the water. The row order follows what the tiles nest inside what: industry
// in housing, farmland in meadows, woods in fields, gardens in campuses and allotments, plazas in
// parks, playgrounds in kindergartens. A kind missing here gets no fill; none occurs in the
// planner bounds.
const land: { tier: Tier; kinds: string[]; light: string; dark: string }[] = [
    { tier: "ground", kinds: ["residential"], light: "#e9e7e2", dark: "#2b2a26" },
    { tier: "ground", kinds: ["commercial"], light: "#eae4e1", dark: "#2c2826" },
    { tier: "ground", kinds: ["industrial"], light: "#dfe0e2", dark: "#282a2f" },
    { tier: "ground", kinds: ["railway"], light: "#e4e1e5", dark: "#2a272b" },
    { tier: "ground", kinds: ["military"], light: "#eae1e1", dark: "#2d2525" },
    { tier: "ground", kinds: ["aerodrome", "airfield"], light: "#e6e6ea", dark: "#28282f" },
    { tier: "ground", kinds: ["other"], light: "#e8e8df", dark: "#2a2a25" },
    { tier: "ground", kinds: ["meadow", "grass", "grassland"], light: "#ecf3e2", dark: "#22261d" },
    { tier: "ground", kinds: ["farmland"], light: "#f5efe0", dark: "#25231d" },
    { tier: "ground", kinds: ["sand", "beach"], light: "#f4edd7", dark: "#2c2920" },
    { tier: "ground", kinds: ["glacier"], light: "#e5eef0", dark: "#3c5558" },
    { tier: "ground", kinds: ["bare_rock"], light: "#e3e1de", dark: "#282725" },
    { tier: "ground", kinds: ["wetland"], light: "#e1efea", dark: "#1f2925" },
    { tier: "ground", kinds: ["scrub"], light: "#e4eedd", dark: "#23291e" },
    { tier: "ground", kinds: ["wood", "forest"], light: "#d2dfc5", dark: "#23392e" },
    { tier: "zone", kinds: ["allotments"], light: "#e7ebd1", dark: "#343725" },
    { tier: "zone", kinds: ["cemetery"], light: "#dee4d8", dark: "#2e3328" },
    { tier: "zone", kinds: ["school", "university", "college"], light: "#f0eadb", dark: "#353127" },
    { tier: "zone", kinds: ["hospital"], light: "#f3e5e2", dark: "#392b28" },
    { tier: "zone", kinds: ["runway", "taxiway"], light: "#d6d6dc", dark: "#383842" },
    { tier: "zone", kinds: ["park", "garden", "village_green", "recreation_ground", "golf_course", "zoo", "dog_park"], light: "#d7eacd", dark: "#2f3e28" },
    { tier: "zone", kinds: ["pedestrian"], light: "#e9e6dd", dark: "#32302a" },
    { tier: "detail", kinds: ["kindergarten"], light: "#f0eadb", dark: "#353127" },
    { tier: "detail", kinds: ["pitch", "playground"], light: "#c6dfb9", dark: "#384d2d" },
    { tier: "structure", kinds: ["platform", "pier", "dam"], light: "#dcdad6", dark: "#3b3935" },
];
// Protected areas lie over other land, so a boundary shows them and a fill would hide it.
const protectedKinds = ["national_park", "nature_reserve", "protected_area"];
// Below zoom 12 the land fades to paper, the strong kinds last; a region view stays calm.
const fade = (strong: string[]): ExpressionSpecification =>
    ["interpolate", ["linear"], ["zoom"], 6, 0, 9, strong.length ? ["match", ["get", "kind"], strong, 0.6, 0] : 0, 12, 1] as unknown as ExpressionSpecification;

function landLayers(dark: boolean): Record<Tier | "protected", LayerSpecification> {
    const byKind = (rows: typeof land, value: (row: (typeof land)[number]) => unknown, fallback: unknown): ExpressionSpecification =>
        ["match", ["get", "kind"], ...rows.flatMap((row) => [row.kinds, value(row)]), fallback] as unknown as ExpressionSpecification;
    const fill = (tier: Tier, opacity?: ExpressionSpecification): LayerSpecification => {
        const rows = land.filter((row) => row.tier === tier);
        return {
            id: `land-${tier}`, type: "fill", source: "basemap", "source-layer": "landuse",
            ...(tier === "detail" || tier === "structure" ? { minzoom: 13 } : {}),
            ...(rows.length > 1 ? { layout: { "fill-sort-key": byKind(rows, (row) => rows.indexOf(row), 0) } } : {}),
            filter: ["in", ["get", "kind"], ["literal", rows.flatMap((row) => row.kinds)]],
            paint: { "fill-color": byKind(rows, (row) => (dark ? row.dark : row.light), "transparent"), ...(opacity ? { "fill-opacity": opacity } : {}) },
        };
    };
    return {
        ground: fill("ground", fade(["wood", "forest"])), zone: fill("zone", fade([])), detail: fill("detail"), structure: fill("structure"),
        protected: {
            id: "land-protected", type: "line", source: "basemap", "source-layer": "landuse", minzoom: 11,
            filter: ["in", ["get", "kind"], ["literal", protectedKinds]],
            paint: { "line-color": dark ? "#86ad72" : "#5f8a4c", "line-opacity": 0.5, "line-dasharray": [4, 2], "line-width": ["interpolate", ["linear"], ["zoom"], 11, 1, 14, 1.6] },
        },
    };
}

/** Cliffs and ferries, which the theme does not draw. */
function cliffAndFerryLayers(dark: boolean): LayerSpecification[] {
    const rock = dark ? "#8d877a" : "#7d766a";
    return [
        {
            id: "cliffs", type: "line", source: "basemap", "source-layer": "earth", minzoom: 12, filter: ["==", ["get", "kind"], "cliff"],
            paint: { "line-color": rock, "line-width": ["interpolate", ["linear"], ["zoom"], 12, 0.6, 16, 1.2] },
        },
        // An OSM cliff way keeps its low side on the right, so the teeth hang off that side.
        {
            id: "cliff-teeth", type: "line", source: "basemap", "source-layer": "earth", minzoom: 13, filter: ["==", ["get", "kind"], "cliff"],
            paint: { "line-color": rock, "line-width": 3, "line-offset": 1.5, "line-dasharray": [0.4, 1.2] },
        },
        {
            id: "ferries", type: "line", source: "basemap", "source-layer": "roads", minzoom: 11, filter: ["==", ["get", "kind"], "ferry"],
            paint: { "line-color": dark ? "#97bdc8" : "#3e6677", "line-width": ["interpolate", ["linear"], ["zoom"], 11, 0.8, 16, 1.5], "line-dasharray": [3, 2] },
        },
    ];
}

/** The theme's layers with its land use layers replaced by the planner's, which draw every kind, and with cliffs and ferries. */
function baseLayers(dark: boolean): LayerSpecification[] {
    const theme = layers("basemap", flavor(dark), { lang: "en" }) as LayerSpecification[];
    const base = theme.filter((layer) => !layer.id.startsWith("landuse_"));
    const { ground, zone, detail, structure, protected: outline } = landLayers(dark);
    // The land takes the theme's land position, under the relief and the water; structures and
    // the protected boundary sit over the water.
    base.splice(theme.findIndex((layer) => layer.id.startsWith("landuse_")), 0, ground, zone, detail);
    base.splice(base.findIndex((layer) => layer.id === "water") + 1, 0, structure, outline);
    base.splice(base.findIndex((layer) => layer.id === "water_river") + 1, 0, ...cliffAndFerryLayers(dark));
    const river = base.find((layer) => layer.id === "water_river");
    if (river?.type === "line") river.filter = ["in", ["get", "kind"], ["literal", ["river", "canal"]]];
    // A region view loads one glyph stack: below zoom 10 the Medium and Italic labels draw in Regular.
    for (const layer of base) {
        if (layer.type !== "symbol" || !layer.layout?.["text-font"]) continue;
        const font = layer.layout["text-font"];
        const stack = Array.isArray(font) && font.every((name) => typeof name === "string") ? ["literal", font] : font;
        layer.layout["text-font"] = ["step", ["zoom"], ["literal", ["Noto Sans Regular"]], 10, stack] as unknown as ExpressionSpecification;
    }
    return base;
}

function flavor(dark: boolean): Flavor {
    const paper = dark ? "#181d19" : "#f4f2eb";
    const local = dark ? "#7c8177" : "#ffffff";
    const casing = dark ? "#22291f" : "#c5c0b3";
    const ink = dark ? "#d6dbcc" : "#414a3a";
    const shade = (kind: string) => { const row = land.find((row) => row.kinds.includes(kind))!; return dark ? row.dark : row.light; };
    return {
        ...namedFlavor(dark ? "dark" : "light"),
        background: paper, earth: paper,
        water: dark ? "#183b4b" : "#a9cbd4",
        buildings: dark ? "#3b4242" : "#c9c3b7",
        minor_a: local, minor_b: local, minor_service: local,
        minor_casing: casing, minor_service_casing: casing,
        other: dark ? "#c3aa77" : "#998363", bridges_other: dark ? "#c3aa77" : "#998363",
        major: dark ? "#c2a269" : "#e9c38b", highway: dark ? "#c98658" : "#dfa879",
        link: dark ? "#c2a269" : "#e9c38b", bridges_major: dark ? "#c2a269" : "#e9c38b",
        bridges_highway: dark ? "#c98658" : "#dfa879", bridges_minor: local,
        major_casing_early: casing, major_casing_late: casing,
        highway_casing_early: casing, highway_casing_late: casing,
        roads_label_minor: ink, roads_label_major: ink, city_label: ink, subplace_label: ink,
        roads_label_minor_halo: paper, roads_label_major_halo: paper,
        city_label_halo: paper, subplace_label_halo: paper,
        ocean_label: dark ? "#97bdc8" : "#3e6677",
        landcover: {
            barren: shade("bare_rock"), forest: shade("forest"), farmland: shade("farmland"), grassland: shade("meadow"),
            scrub: shade("scrub"), urban_area: shade("residential"), glacier: shade("glacier"),
        },
    };
}

/** The shared basemap omits points of interest and planner overlays. */
export function basemapStyle(theme: "light" | "dark", config: BasemapConfig): StyleSpecification {
    return {
        version: 8,
        glyphs: config.glyphs,
        sprite: `${config.sprites}/${theme}`,
        sources: { basemap: basemapSource(config) },
        layers: baseLayers(theme === "dark").filter((layer) => layer.id !== "pois"),
    };
}

/** Relief shading; over the snow layer, shadows turn blue-grey so snow reads as snow. */
export function reliefPaint(dark: boolean, snow = false) {
    return {
        "hillshade-exaggeration": dark ? 0.31 : snow ? 0.26 : 0.2,
        "hillshade-shadow-color": snow ? (dark ? "#070b12" : "#3f5770") : dark ? "#0c120f" : "#657363",
        "hillshade-highlight-color": snow ? (dark ? "#8a98a6" : "#ffffff") : dark ? "#6c7d68" : "#fffdf5",
        "hillshade-accent-color": snow ? (dark ? "#1f2d3a" : "#8297ad") : dark ? "#26392f" : "#99a58c",
    };
}

export function mapStyle(theme: "light" | "dark", config: PlannerConfig, demUrl: string, contourUrl: string): StyleSpecification {
    const dark = theme === "dark";
    const base = baseLayers(dark);
    const afterLand = base.findIndex((layer) => layer.id === "land-detail") + 1;
    const terrain: LayerSpecification[] = [
        {
            id: "relief", type: "hillshade", source: "terrain",
            paint: {
                ...reliefPaint(dark),
                "hillshade-illumination-direction": 315,
                "hillshade-illumination-anchor": "map",
            },
        },
        {
            id: "contour-lines", type: "line", source: "contours", "source-layer": "contours", minzoom: 10,
            paint: {
                "line-color": dark ? "#b7b184" : "#647f73",
                "line-opacity": dark ? 0.38 : 0.44,
                "line-width": ["match", ["get", "level"], 1, 0.9, 0.45],
            },
        },
    ];
    base.splice(afterLand, 0, ...terrain);
    const firstLabel = base.findIndex((layer) => layer.type === "symbol");
    base.splice(firstLabel, 0, {
        id: "obc-cycleways", type: "line", source: "basemap", "source-layer": "roads", minzoom: 12,
        filter: ["==", ["get", "kind_detail"], "cycleway"],
        paint: { "line-color": dark ? "#b39de8" : "#7762b1", "line-width": ["interpolate", ["linear"], ["zoom"], 12, 1.4, 16, 2.8] },
    });
    // The planner draws its own place kinds; the basemap keeps the rest.
    const pois = base.find((layer) => layer.id === "pois");
    if (pois?.type === "symbol") {
        pois.filter = ["all", pois.filter as ExpressionSpecification, ["!", poiFilter(categoryIds)]];
        if (pois.layout?.["icon-image"]) {
            // The sprite atlas supplies a building glyph for town halls.
            pois.layout["icon-image"] = ["match", ["get", "kind"], "townhall", "building", pois.layout["icon-image"] as ExpressionSpecification];
        }
    }
    const panel = dark ? "#201f17" : "#ffffff";
    const category = ["match", ["get", "kind"], ...categoryIds.flatMap((id) => [Object.keys(placeCategories[id].kinds), id]), ""] as unknown as ExpressionSpecification;
    base.push({
        id: "planner-pois", type: "circle", source: "basemap", "source-layer": "pois", minzoom: 12, maxzoom: 13, filter: poiFilter(categoryIds),
        paint: { "circle-radius": 3.5, "circle-color": panel, "circle-stroke-color": dark ? "#aaa383" : "#676443", "circle-stroke-width": 1.5 },
    }, {
        id: "planner-poi-icons", type: "symbol", source: "basemap", "source-layer": "pois", minzoom: 13, filter: poiFilter(categoryIds),
        layout: {
            "icon-image": ["concat", "poi-", category, `-${theme}`], "icon-padding": 1,
            "text-field": ["step", ["zoom"], "", 14, ["coalesce", ["get", "name:en"], ["get", "name"]]],
            "text-font": ["Noto Sans Regular"], "text-size": 11, "text-anchor": "top", "text-offset": [0, 0.9], "text-optional": true,
        },
        paint: { "text-color": dark ? "#f2efe3" : "#1c1b14", "text-halo-color": panel, "text-halo-width": 1.2 },
    });
    base.push({
        id: "contour-labels", type: "symbol", source: "contours", "source-layer": "contours", minzoom: 12,
        filter: ["==", ["get", "level"], 1],
        layout: { "symbol-placement": "line", "symbol-spacing": 500, "text-field": ["concat", ["to-string", ["get", "ele"]], " m"], "text-font": ["Noto Sans Regular"], "text-size": 11 },
        paint: { "text-color": dark ? "#b7b184" : "#596f65", "text-halo-color": dark ? "#20271e" : "#f4f2eb", "text-halo-width": 1 },
    });
    return {
        version: 8,
        glyphs: config.glyphs,
        sprite: `${config.sprites}/${theme}`,
        sources: {
            basemap: basemapSource(config),
            terrain: { type: "raster-dem", tiles: [demUrl], bounds: config.bounds, tileSize: DEM_TILE, encoding: "terrarium", maxzoom: DEM_MAX_ZOOM, attribution: config.terrain_attribution },
            contours: { type: "vector", tiles: [contourUrl], bounds: config.bounds, maxzoom: 15, attribution: config.terrain_attribution },
        },
        layers: base,
    };
}

/** Matches basemap places of the given categories. */
export function poiFilter(categories: PlaceCategory[]): ExpressionSpecification {
    return ["in", ["get", "kind"], ["literal", Object.keys(poiKinds).filter((kind) => categories.includes(poiKinds[kind].category))]];
}
