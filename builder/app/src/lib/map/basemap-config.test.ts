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
            attribution: "Test map data",
        };
        const fetch = vi.fn().mockResolvedValue(Response.json({ format: 1, active }));
        vi.stubGlobal("fetch", fetch);
        const { basemapConfig } = await import("./basemap-config");
        const { basemapStyle } = await import("../planner/map-style");
        const config = await basemapConfig();
        expect(fetch).toHaveBeenCalledWith("https://maps.openbikecomputer.com/planner/catalog.json", { cache: "no-cache" });
        expect(config).toEqual(active);
        for (const theme of ["light", "dark"] as const) {
            const style = basemapStyle(theme, config);
            expect(validateStyleMin(style)).toEqual([]);
            expect(style.sources.basemap).toHaveProperty("url", active.basemap);
            expect((style.sources.basemap as { attribution: string }).attribution).toContain(active.attribution);
            expect(style.glyphs).toBe(active.glyphs);
            expect(style.sprite).toBe(`${active.sprites}/${theme}`);
        }
    });

    it("uses the planner config that the build carries, with local paths on the page origin", async () => {
        vi.stubGlobal("location", { href: "http://localhost:4175/" });
        vi.stubEnv("VITE_PLANNER_CONFIG", JSON.stringify({ name: "Local", basemap: "pmtiles:///@fs/data/maps/basemap.pmtiles",
            glyphs: "/@fs/data/maps/assets/fonts/{fontstack}/{range}.pbf", sprites: "/@fs/data/maps/assets/sprites/v4",
            attribution: "Local map data" }));
        const fetch = vi.fn();
        vi.stubGlobal("fetch", fetch);
        const { basemapConfig } = await import("./basemap-config");
        expect(await basemapConfig()).toEqual({ basemap: "pmtiles://http://localhost:4175/@fs/data/maps/basemap.pmtiles",
            glyphs: "http://localhost:4175/@fs/data/maps/assets/fonts/{fontstack}/{range}.pbf", sprites: "http://localhost:4175/@fs/data/maps/assets/sprites/v4",
            attribution: "Local map data" });
        expect(fetch).not.toHaveBeenCalled();
    });

    it.each([
        { response: Response.json({ format: 1, active: null }), message: "no active release" },
        { response: Response.json({ format: 1, active: {} }), message: "no basemap" },
        { response: new Response("", { status: 503 }), message: "catalogue is unavailable" },
    ])("rejects an unavailable release: $message", async ({ response, message }) => {
        vi.stubEnv("VITE_PLANNER_CONFIG", "");
        vi.stubGlobal("fetch", vi.fn().mockResolvedValue(response));
        const { basemapConfig } = await import("./basemap-config");
        await expect(basemapConfig()).rejects.toThrow(message);
    });
});
