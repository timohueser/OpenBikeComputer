import { afterEach, describe, expect, it, vi } from "vitest";
import { validateStyleMin } from "@maplibre/maplibre-gl-style-spec";

afterEach(() => {
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
    vi.resetModules();
});

describe("shared basemap hosting", () => {
    it("uses the active hosted release without build settings", async () => {
        vi.stubGlobal("window", { location: { href: "tauri://localhost/" } });
        const active = {
            basemap: "https://tiles.openbikecomputer.com/releases/id/basemap.json",
            glyphs: "https://maps.openbikecomputer.com/releases/id/fonts/{fontstack}/{range}.pbf",
            sprites: "https://maps.openbikecomputer.com/releases/id/sprites",
        };
        const fetch = vi.fn().mockResolvedValue(Response.json({ active }));
        vi.stubGlobal("fetch", fetch);
        const { basemapConfig } = await import("./basemap-config");
        const { basemapStyle } = await import("../planner/map-style");
        const config = await basemapConfig();
        expect(fetch).toHaveBeenCalledWith("https://maps.openbikecomputer.com/planner/catalog.json");
        expect(config).toEqual(active);
        for (const theme of ["light", "dark"] as const) {
            const style = basemapStyle(theme, config);
            expect(validateStyleMin(style)).toEqual([]);
            expect(style.sources.basemap).toHaveProperty("url", active.basemap);
            expect(style.glyphs).toBe(active.glyphs);
            expect(style.sprite).toBe(`${active.sprites}/${theme}`);
        }
    });

    it.each(["VITE_PLANNER_TILEJSON_URL", "VITE_PLANNER_PMTILES_URL"])("keeps explicit %s settings for local previews", async (variable) => {
        vi.stubGlobal("window", { location: { href: "http://localhost:4175/preview/" } });
        vi.stubEnv(variable, variable.endsWith("TILEJSON_URL") ? "./basemap.json" : "./basemap.pmtiles");
        const fetch = vi.fn();
        vi.stubGlobal("fetch", fetch);
        const { basemapConfig } = await import("./basemap-config");
        const { BASEMAP_URL, GLYPHS_URL, SPRITES_URL } = await import("../planner/map-data");
        expect(await basemapConfig()).toEqual({ basemap: BASEMAP_URL, glyphs: GLYPHS_URL, sprites: SPRITES_URL });
        expect(fetch).not.toHaveBeenCalled();
    });

    it.each([Response.json({ active: null }), new Response("", { status: 503 })])("rejects an unavailable release", async (response) => {
        vi.stubGlobal("window", { location: { href: "tauri://localhost/" } });
        vi.stubGlobal("fetch", vi.fn().mockResolvedValue(response));
        const { basemapConfig } = await import("./basemap-config");
        await expect(basemapConfig()).rejects.toThrow("Basemap catalog");
    });
});
