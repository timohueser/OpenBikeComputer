import { BASEMAP_URL, GLYPHS_URL, SPRITES_URL } from "../planner/map-data";

export interface BasemapConfig {
    basemap: string;
    glyphs: string;
    sprites: string;
}

/** Local planner previews use build settings; ordinary builds use the active release. */
export async function basemapConfig(): Promise<BasemapConfig> {
    if (import.meta.env.VITE_PLANNER_TILEJSON_URL || import.meta.env.VITE_PLANNER_PMTILES_URL) {
        return { basemap: BASEMAP_URL, glyphs: GLYPHS_URL, sprites: SPRITES_URL };
    }
    const response = await fetch("https://maps.openbikecomputer.com/planner/catalog.json");
    if (!response.ok) throw new Error(`Basemap catalog: HTTP ${response.status}`);
    const { active } = await response.json();
    if (![active?.basemap, active?.glyphs, active?.sprites].every((value) => typeof value === "string" && value.startsWith("https://"))) {
        throw new Error("Basemap catalog has no active map endpoints.");
    }
    return { basemap: active.basemap, glyphs: active.glyphs, sprites: active.sprites };
}
