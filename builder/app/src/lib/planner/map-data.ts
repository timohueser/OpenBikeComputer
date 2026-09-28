import type { Coordinate } from "./map-types";

// Public evaluation sources. Replace with owned, versioned extracts before deployment.
const archive = import.meta.env.VITE_PLANNER_PMTILES_URL || "https://demo-bucket.protomaps.com/v4.pmtiles";
export const BASEMAP_URL = archive.startsWith("pmtiles://") ? archive : `pmtiles://${new URL(archive, window.location.href).href}`;
export const TERRAIN_URL = import.meta.env.VITE_PLANNER_DEM_URL || "https://tiles.mapterhorn.com/{z}/{x}/{y}.webp";
export const MAP_VIEWS: { name: string; center: Coordinate; zoom: number }[] = [
    { name: "Grindelwald · mountain detail", center: [8.035, 46.625], zoom: 13 },
    { name: "Interlaken · valley and town", center: [7.86, 46.685], zoom: 12 },
    { name: "Bern · street detail", center: [7.447, 46.948], zoom: 14 },
    { name: "Swiss Alps · overview", center: [7.6, 46.9], zoom: 9 },
];
