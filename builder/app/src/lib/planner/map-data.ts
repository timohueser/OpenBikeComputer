import { clientConfig } from "./client-config";
import type { Archive } from "./layers/data-layer";
import type { Coordinate } from "./map-types";

function absoluteUrl(value: string): string {
    // Contour workers need absolute URLs; MapLibre expands the template tokens.
    return new URL(value, window.location.href).href.replace(/%7B/g, "{").replace(/%7D/g, "}");
}

const archive = import.meta.env.VITE_PLANNER_PMTILES_URL || "./data/planner/basemap.pmtiles";
export const BASEMAP_URL = import.meta.env.VITE_PLANNER_TILEJSON_URL
    ? absoluteUrl(import.meta.env.VITE_PLANNER_TILEJSON_URL)
    : `pmtiles://${absoluteUrl(archive.replace(/^pmtiles:\/\//, ""))}`;
/** Rider places: a TileJSON URL ending in `.json`, or a PMTiles archive. */
export const PLACES_URL = absoluteUrl(import.meta.env.VITE_PLANNER_PLACES_URL || "./data/planner/places.pmtiles");
/** Route networks and access: a TileJSON URL ending in `.json`, or a PMTiles archive. */
export const OVERLAYS_URL = absoluteUrl(import.meta.env.VITE_PLANNER_OVERLAYS_URL || "./data/planner/overlays.pmtiles");
const dataUrl = (value: string | undefined) => value ? absoluteUrl(value) : "";
/** Data layer archives, such as the snow history of specs/planner-snow-tiles.md: a TileJSON URL ending in `.json`, a PMTiles archive, or empty when the region has none. */
export const DATA_URLS: Record<Archive, string> = {
    snow: dataUrl(import.meta.env.VITE_PLANNER_SNOW_URL),
    climate: dataUrl(import.meta.env.VITE_PLANNER_CLIMATE_URL),
};
export const TERRAIN_URL = absoluteUrl(import.meta.env.VITE_PLANNER_DEM_URL || "./tiles/terrain/{z}/{x}/{y}.webp");
export const GLYPHS_URL = absoluteUrl(import.meta.env.VITE_PLANNER_GLYPHS_URL || "./data/planner/assets/fonts/{fontstack}/{range}.pbf");
export const SPRITES_URL = absoluteUrl(import.meta.env.VITE_PLANNER_SPRITES_URL || "./data/planner/assets/sprites/v4");
export const TERRAIN_ATTRIBUTION = import.meta.env.VITE_PLANNER_TERRAIN_ATTRIBUTION || '<a href="https://mapterhorn.com/attribution">Terrain © Mapterhorn contributors</a>';
function mapBounds(value: string | undefined): [number, number, number, number] | undefined {
    if (!value) return undefined;
    const coordinates = value.split(",").map(Number);
    const [west, south, east, north] = coordinates;
    if (coordinates.length !== 4 || !coordinates.every(Number.isFinite)
        || west < -180 || east > 180 || south < -85 || north > 85 || west >= east || south >= north) {
        throw new Error("VITE_PLANNER_MAP_BOUNDS must be west,south,east,north in degrees.");
    }
    return coordinates as [number, number, number, number];
}

export const MAP_BOUNDS = mapBounds(clientConfig.bounds?.join(",") ?? import.meta.env.VITE_PLANNER_MAP_BOUNDS);
export const MAP_VIEWS: { name: string; center: Coordinate; zoom: number }[] = [
    { name: "Freiburg · street detail", center: [7.849, 47.997], zoom: 14 },
    { name: "Feldberg · mountain detail", center: [8.005, 47.873], zoom: 13 },
    { name: "Stuttgart · city", center: [9.182, 48.775], zoom: 12 },
    { name: "Baden-Württemberg · overview", center: [8.95, 48.65], zoom: 7 },
];
