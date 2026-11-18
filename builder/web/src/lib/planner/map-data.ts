import { plannerConfig, type Bounds } from "./config";
import { activeRelease, pageRelease, releaseFetch } from "./release";
import type { CatalogRecord, TrailEntry } from "./signed-routes";

const preview: string | undefined = import.meta.env.VITE_PLANNER_CONFIG;

/** The planner config: the one that a local preview build carries, else the active release at page load. */
export const config = plannerConfig(preview ? JSON.parse(preview) : await activeRelease());
pageRelease(config.id);

/** Whether the zoom 9 cell `9-X-Y`, or the zoom 7 cell `7-X-Y`, overlaps the bounds with a positive area. */
export function coversCell(bounds: Bounds, id: string): boolean {
    const match = /^([79])-(\d+)-(\d+)$/.exec(id);
    const n = 2 ** Number(match?.[1]), x = Number(match?.[2]), y = Number(match?.[3]);
    if (!match || x >= n || y >= n) throw new Error(`Invalid route catalog cell ${id}`);
    const latitude = (row: number) => Math.atan(Math.sinh(Math.PI * (1 - 2 * row / n))) * 180 / Math.PI;
    // Cell-aligned bounds are the edges of their cells. The margin keeps a neighbour that only touches
    // such an edge outside, also when its edge latitude differs from the release builder's in the last bit.
    const margin = 1e-9;
    return Math.min(bounds[2], (x + 1) / n * 360 - 180) - Math.max(bounds[0], x / n * 360 - 180) > margin
        && Math.min(bounds[3], latitude(y)) - Math.max(bounds[1], latitude(y + 1)) > margin;
}

/** The list `key` of a route catalog file. */
async function catalogFile<T>(url: string, key: "routes" | "trails"): Promise<T[]> {
    const response = await releaseFetch(url);
    if (!response.ok) throw new Error(`Route catalog request failed with status ${response.status}`);
    const document = await response.json();
    if (document?.format !== 1 || !Array.isArray(document[key])) throw new Error("Unsupported route catalog");
    return document[key];
}

const routeFile = (url: string) => catalogFile<CatalogRecord>(url, "routes");

let regionRoutes: Promise<CatalogRecord[]> | undefined;

function regionFile(url: string): Promise<CatalogRecord[]> {
    regionRoutes ??= routeFile(url).catch((error) => {
        regionRoutes = undefined;
        throw error;
    });
    return regionRoutes;
}

/**
 * The catalog records of the zoom 9 cell `9-X-Y`. A covered cell with no routes gives an empty list.
 * A cell outside the release bounds gives null and is not fetched.
 */
export async function loadRouteCell(id: string): Promise<CatalogRecord[] | null> {
    const url = config.routes;
    if (!url || !coversCell(config.bounds, id)) return null;
    if (url.includes("{cell}")) return routeFile(url.replace("{cell}", id));
    return (await regionFile(url)).filter((record) => record.cells.includes(id));
}

/**
 * The trails of the far file of the zoom 7 cell `7-X-Y`. A covered cell with no trails gives an empty list.
 * A cell outside the release bounds, or a release with no trail files, gives null and is not fetched.
 */
export async function loadTrailCell(id: string): Promise<TrailEntry[] | null> {
    const url = config.trails;
    if (!url || !coversCell(config.bounds, id)) return null;
    return catalogFile<TrailEntry>(url.replace("{cell}", id), "trails");
}

/** The whole record of the trail `id`: its trail record file, or the record of the region file of a regional release. */
export async function loadTrailRecord(id: number): Promise<CatalogRecord | null> {
    const { trail, routes } = config;
    const records = trail ? await routeFile(trail.replace("{id}", String(id)))
        : routes && !routes.includes("{cell}") ? await regionFile(routes) : null;
    return records?.find((record) => record.id === id) ?? null;
}
