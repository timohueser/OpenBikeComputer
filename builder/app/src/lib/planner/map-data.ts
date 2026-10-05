import { plannerConfig, type Bounds } from "./config";
import type { Coordinate } from "./map-types";
import { activeRelease, pageRelease, releaseFetch } from "./release";
import type { CatalogRecord } from "./signed-routes";

const preview: string | undefined = import.meta.env.VITE_PLANNER_CONFIG;

/** The planner config: the one that a local preview build carries, else the active release at page load. */
export const config = plannerConfig(preview ? JSON.parse(preview) : await activeRelease(), globalThis.location?.href);
pageRelease(config.id);

export const MAP_VIEWS: { name: string; center: Coordinate; zoom: number }[] = [
    { name: "Freiburg · street detail", center: [7.849, 47.997], zoom: 14 },
    { name: "Feldberg · mountain detail", center: [8.005, 47.873], zoom: 13 },
    { name: "Stuttgart · city", center: [9.182, 48.775], zoom: 12 },
    { name: "Baden-Württemberg · overview", center: [8.95, 48.65], zoom: 7 },
];

/** Whether the zoom 9 cell `9-X-Y` overlaps the bounds with a positive area. */
export function coversCell(bounds: Bounds, id: string): boolean {
    const match = /^9-(\d+)-(\d+)$/.exec(id);
    const n = 512, x = Number(match?.[1]), y = Number(match?.[2]);
    if (!match || x >= n || y >= n) throw new Error(`Invalid route catalog cell ${id}`);
    const latitude = (row: number) => Math.atan(Math.sinh(Math.PI * (1 - 2 * row / n))) * 180 / Math.PI;
    // Cell-aligned bounds are the edges of their cells. The margin keeps a neighbour that only touches
    // such an edge outside, also when its edge latitude differs from the release builder's in the last bit.
    const margin = 1e-9;
    return Math.min(bounds[2], (x + 1) / n * 360 - 180) - Math.max(bounds[0], x / n * 360 - 180) > margin
        && Math.min(bounds[3], latitude(y)) - Math.max(bounds[1], latitude(y + 1)) > margin;
}

async function routeFile(url: string): Promise<CatalogRecord[]> {
    const response = await releaseFetch(url);
    if (!response.ok) throw new Error(`Route catalog request failed with status ${response.status}`);
    const document = await response.json();
    if (document?.format !== 1 || !Array.isArray(document.routes)) throw new Error("Unsupported route catalog");
    return document.routes;
}

let regionRoutes: Promise<CatalogRecord[]> | undefined;

/**
 * The catalog records of the zoom 9 cell `9-X-Y`. A covered cell with no routes gives an empty list.
 * A cell outside the release bounds gives null and is not fetched.
 */
export async function loadRouteCell(id: string): Promise<CatalogRecord[] | null> {
    const url = config.routes;
    if (!url || !coversCell(config.bounds, id)) return null;
    if (url.includes("{cell}")) return routeFile(url.replace("{cell}", id));
    regionRoutes ??= routeFile(url).catch((error) => {
        regionRoutes = undefined;
        throw error;
    });
    return (await regionRoutes).filter((record) => record.cells.includes(id));
}
