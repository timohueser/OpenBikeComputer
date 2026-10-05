import { configUrl, type PlannerConfig } from "../planner/config";
import { activeRelease } from "../planner/release";

export type BasemapConfig = Pick<PlannerConfig, "basemap" | "glyphs" | "sprites">;

/** The basemap of the planner config that this build carries, else of the live catalogue's active release. */
export async function basemapConfig(): Promise<BasemapConfig> {
    const preview: string | undefined = import.meta.env.VITE_PLANNER_CONFIG;
    const active = preview ? JSON.parse(preview) : await activeRelease();
    const [basemap, glyphs, sprites] = [active.basemap, active.glyphs, active.sprites].map((value) => configUrl(value, globalThis.location?.href));
    if (!basemap || !glyphs || !sprites) throw new Error("The active release has no basemap.");
    return { basemap, glyphs, sprites };
}
