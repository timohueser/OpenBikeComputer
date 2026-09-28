import { afterEach, describe, expect, it, vi } from 'vitest';
import { validateStyleMin } from '@maplibre/maplibre-gl-style-spec';

afterEach(() => {
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
    vi.resetModules();
});

describe('planner map hosting', () => {
    it('keeps all default map requests on the host, including a mounted preview', async () => {
        vi.stubGlobal('window', { location: { href: 'http://localhost:4175/preview/planner.html' } });
        const data = await import('./map-data');
        const { mapStyle } = await import('./map-style');
        expect(data.BASEMAP_URL).toBe('pmtiles://http://localhost:4175/preview/data/planner/basemap.pmtiles');
        expect(data.TERRAIN_URL).toBe('http://localhost:4175/preview/tiles/terrain/{z}/{x}/{y}.webp');
        for (const theme of ['light', 'dark'] as const) {
            const style = mapStyle(theme, 'dem://tiles', 'contours://tiles');
            expect(validateStyleMin(style)).toEqual([]);
            expect(style.glyphs).toBe('http://localhost:4175/preview/data/planner/assets/fonts/{fontstack}/{range}.pbf');
            expect(style.sprite).toBe(`http://localhost:4175/preview/data/planner/assets/sprites/v4/${theme}`);
        }
    });

    it('uses the configured bucket, terrain endpoint and regional coverage together', async () => {
        vi.stubGlobal('window', { location: { href: 'https://planner.example/planner.html' } });
        vi.stubEnv('VITE_PLANNER_PMTILES_URL', 'pmtiles://https://maps.example/region/basemap.pmtiles');
        vi.stubEnv('VITE_PLANNER_DEM_URL', 'https://maps.example/terrain/{z}/{x}/{y}.webp');
        vi.stubEnv('VITE_PLANNER_GLYPHS_URL', 'https://maps.example/assets/fonts/{fontstack}/{range}.pbf');
        vi.stubEnv('VITE_PLANNER_SPRITES_URL', 'https://maps.example/assets/sprites/v4');
        vi.stubEnv('VITE_PLANNER_MAP_BOUNDS', '7.45,47.5,10.5,49.85');
        const data = await import('./map-data');
        const { mapStyle } = await import('./map-style');
        const style = mapStyle('dark', 'dem://tiles', 'contours://tiles');
        expect(validateStyleMin(style)).toEqual([]);
        expect(data.BASEMAP_URL).toBe('pmtiles://https://maps.example/region/basemap.pmtiles');
        expect(data.TERRAIN_URL).toBe('https://maps.example/terrain/{z}/{x}/{y}.webp');
        expect(style.glyphs).toBe('https://maps.example/assets/fonts/{fontstack}/{range}.pbf');
        expect(style.sprite).toBe('https://maps.example/assets/sprites/v4/dark');
        expect(style.sources.terrain).toHaveProperty('bounds', [7.45, 47.5, 10.5, 49.85]);
        expect(style.sources.contours).toHaveProperty('bounds', data.MAP_BOUNDS);
    });
});
