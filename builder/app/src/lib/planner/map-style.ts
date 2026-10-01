import { layers, namedFlavor, type Flavor } from "@protomaps/basemaps";
import type { ExpressionSpecification, StyleSpecification, LayerSpecification } from "maplibre-gl";
import { BASEMAP_URL, GLYPHS_URL, MAP_BOUNDS, SPRITES_URL, TERRAIN_ATTRIBUTION } from "./map-data";
import { categoryIds, placeCategories, poiKinds, type PlaceCategory } from "./poi-kinds";

const BASEMAP_SOURCE = {
    type: "vector",
    url: BASEMAP_URL,
    attribution: '<a href="https://openstreetmap.org/copyright">© OpenStreetMap contributors</a> · <a href="https://protomaps.com">Protomaps</a>',
} as const;

function flavor(dark: boolean): Flavor {
    const paper = dark ? "#181d19" : "#f4f2eb";
    const wood = dark ? "#23392e" : "#d2dfc5";
    const field = dark ? "#303827" : "#e5e9d5";
    const local = dark ? "#7c8177" : "#ffffff";
    const casing = dark ? "#22291f" : "#c5c0b3";
    const ink = dark ? "#d6dbcc" : "#414a3a";
    return {
        ...namedFlavor(dark ? "dark" : "light"),
        background: paper, earth: paper, wood_a: wood, wood_b: wood,
        park_a: field, park_b: field, scrub_a: field, scrub_b: field,
        water: dark ? "#183b4b" : "#a9cbd4", glacier: dark ? "#3c5558" : "#e5eef0",
        buildings: dark ? "#3b4242" : "#c9c3b7", industrial: dark ? "#292d32" : "#e4e0d7",
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
            barren: dark ? "#363833" : "#dddcd3", forest: wood,
            farmland: field, grassland: field, scrub: field,
            urban_area: dark ? "#292d32" : "#e4e0d7", glacier: dark ? "#3c5558" : "#e5eef0",
        },
    };
}

/** The planner's basemap alone, for a picker that only places rectangles: no points of interest, terrain or cycleways. */
export function basemapStyle(): StyleSpecification {
    return {
        version: 8,
        glyphs: GLYPHS_URL,
        sprite: `${SPRITES_URL}/light`,
        sources: { basemap: BASEMAP_SOURCE },
        layers: (layers("basemap", flavor(false), { lang: "en" }) as LayerSpecification[]).filter((layer) => layer.id !== "pois"),
    };
}

export function mapStyle(theme: "light" | "dark", demUrl: string, contourUrl: string): StyleSpecification {
    const dark = theme === "dark";
    const base = layers("basemap", flavor(dark), { lang: "en" }) as LayerSpecification[];
    const firstRoad = base.findIndex((layer) => layer.id.startsWith("roads"));
    const terrain: LayerSpecification[] = [
        {
            id: "relief", type: "hillshade", source: "terrain",
            paint: {
                "hillshade-exaggeration": dark ? 0.28 : 0.2,
                "hillshade-shadow-color": dark ? "#0c120f" : "#657363",
                "hillshade-highlight-color": dark ? "#6c7d68" : "#fffdf5",
                "hillshade-accent-color": dark ? "#26392f" : "#99a58c",
                "hillshade-illumination-direction": 315,
                "hillshade-illumination-anchor": "map",
            },
        },
        {
            id: "contour-lines", type: "line", source: "contours", "source-layer": "contours", minzoom: 10,
            paint: {
                "line-color": dark ? "#b7b184" : "#647f73",
                "line-opacity": dark ? 0.38 : 0.4,
                "line-width": ["match", ["get", "level"], 1, 0.9, 0.45],
            },
        },
    ];
    base.splice(firstRoad < 0 ? 2 : firstRoad, 0, ...terrain);
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
        glyphs: GLYPHS_URL,
        sprite: `${SPRITES_URL}/${theme}`,
        sources: {
            basemap: BASEMAP_SOURCE,
            terrain: { type: "raster-dem", tiles: [demUrl], ...(MAP_BOUNDS ? { bounds: MAP_BOUNDS } : {}), tileSize: 512, encoding: "terrarium", maxzoom: 12, attribution: TERRAIN_ATTRIBUTION },
            contours: { type: "vector", tiles: [contourUrl], ...(MAP_BOUNDS ? { bounds: MAP_BOUNDS } : {}), maxzoom: 15, attribution: TERRAIN_ATTRIBUTION },
        },
        layers: base,
    };
}

/** Matches basemap places of the given categories. */
export function poiFilter(categories: PlaceCategory[]): ExpressionSpecification {
    return ["in", ["get", "kind"], ["literal", Object.keys(poiKinds).filter((kind) => categories.includes(poiKinds[kind].category))]];
}
