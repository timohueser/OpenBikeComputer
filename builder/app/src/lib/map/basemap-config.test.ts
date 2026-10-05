import { afterEach, describe, expect, it, vi } from "vitest";
import { validateStyleMin } from "@maplibre/maplibre-gl-style-spec";

afterEach(() => {
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
    vi.resetModules();
});

describe("shared basemap hosting", () => {
    it("uses the active hosted release in a build without a planner config", async () => {
        vi.stubEnv("VITE_PLANNER_CONFIG", "");
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

    it("uses the planner config that the build carries", async () => {
        const local = { basemap: "pmtiles://http://127.0.0.1:4175/@fs/data/maps/basemap.pmtiles",
            glyphs: "http://127.0.0.1:4175/@fs/data/maps/assets/fonts/{fontstack}/{range}.pbf", sprites: "http://127.0.0.1:4175/@fs/data/maps/assets/sprites/v4" };
        vi.stubEnv("VITE_PLANNER_CONFIG", JSON.stringify({ name: "Local", ...local }));
        const fetch = vi.fn();
        vi.stubGlobal("fetch", fetch);
        const { basemapConfig } = await import("./basemap-config");
        expect(await basemapConfig()).toEqual(local);
        expect(fetch).not.toHaveBeenCalled();
    });

    it.each([Response.json({ active: null }), new Response("", { status: 503 })])("rejects an unavailable release", async (response) => {
        vi.stubEnv("VITE_PLANNER_CONFIG", "");
        vi.stubGlobal("fetch", vi.fn().mockResolvedValue(response));
        const { basemapConfig } = await import("./basemap-config");
        await expect(basemapConfig()).rejects.toThrow("Basemap catalog");
    });
});
