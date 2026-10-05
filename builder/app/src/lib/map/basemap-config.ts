import { configUrl, type PlannerConfig } from "../planner/config";

export type BasemapConfig = Pick<PlannerConfig, "basemap" | "glyphs" | "sprites">;

/** The basemap of the planner config that this build carries, else of the live catalogue's active release. */
export async function basemapConfig(): Promise<BasemapConfig> {
    let active;
    if (import.meta.env.VITE_PLANNER_CONFIG) {
        active = JSON.parse(import.meta.env.VITE_PLANNER_CONFIG);
    } else {
        const response = await fetch("https://maps.openbikecomputer.com/planner/catalog.json");
        if (!response.ok) throw new Error(`Basemap catalog: HTTP ${response.status}`);
        active = (await response.json()).active;
    }
    const [basemap, glyphs, sprites] = [active?.basemap, active?.glyphs, active?.sprites].map((value) => configUrl(value, globalThis.location?.href));
    if (!basemap || !glyphs || !sprites) throw new Error("Basemap catalog has no active map endpoints.");
    return { basemap, glyphs, sprites };
}
