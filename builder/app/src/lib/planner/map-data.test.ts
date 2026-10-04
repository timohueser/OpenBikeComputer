import { afterEach, describe, expect, it, vi } from 'vitest';
import { validateStyleMin } from '@maplibre/maplibre-gl-style-spec';

afterEach(() => {
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
    vi.resetModules();
});

describe('planner map hosting', () => {
    it('uses cached XYZ tiles for the hosted vector map', async () => {
        vi.stubGlobal('window', { location: { href: 'https://planner.example/plan/' } });
        vi.stubEnv('VITE_PLANNER_TILEJSON_URL', 'https://tiles.example/releases/id/basemap.json');
        const data = await import('./map-data');
        const { mapStyle } = await import('./map-style');
        expect(data.BASEMAP_URL).toBe('https://tiles.example/releases/id/basemap.json');
        expect(mapStyle('light', 'dem://tiles', 'contours://tiles').sources.basemap).toHaveProperty('url', data.BASEMAP_URL);
    });
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

    it('gives Leaflet maps the basemap alone, without places, terrain or cycleways', async () => {
        vi.stubGlobal('window', { location: { href: 'https://planner.example/builder/' } });
        const { basemapStyle } = await import('./map-style');
        for (const theme of ['light', 'dark'] as const) {
            const style = basemapStyle(theme);
            expect(validateStyleMin(style)).toEqual([]);
            expect(style.sprite).toMatch(new RegExp(`/${theme}$`));
            expect(Object.keys(style.sources)).toEqual(['basemap']);
            expect(style.layers.map((layer) => layer.id)).not.toContain('pois');
            expect(style.layers.some((layer) => layer.type === 'background')).toBe(true);
            expect(style.layers.every((layer) => !('source' in layer) || layer.source === 'basemap')).toBe(true);
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

// Cell-aligned bounds of the cells 9-267-178 and 9-268-178, as the release builder writes them.
const BOUNDS = '7.734375,47.51720069783939,9.140625,47.98992166741417';
const record = (id: number, cells: string[]) => ({ id, kind: 'hiking', name: `Route ${id}`, rank: 1, loop: false,
    length_m: 3000, ascent_m: 10, descent_m: 10, cells, line_udeg: [8000000, 47800000, 100, 100], via: [] });

async function catalog(url: string, files: Record<string, unknown>) {
    vi.stubGlobal('window', { location: { href: 'https://planner.example/planner.html' } });
    vi.stubEnv('VITE_PLANNER_ROUTES_URL', url);
    vi.stubEnv('VITE_PLANNER_MAP_BOUNDS', BOUNDS);
    const fetch = vi.fn(async (url: string) => url in files
        ? new Response(JSON.stringify(files[url])) : new Response('Archive not found', { status: 404 }));
    vi.stubGlobal('fetch', fetch);
    return { fetch, loadRouteCell: (await import('./map-data')).loadRouteCell };
}

describe('route catalog cells', () => {
    it('reads a covered grid cell, and never fetches a cell outside the covered set', async () => {
        const tiles = 'https://tiles.example/releases/id/routes/tiles';
        const loop = { ...record(7, ['9-267-178']), loop: true };
        const { fetch, loadRouteCell } = await catalog(`${tiles}/{cell}.json`, {
            [`${tiles}/9-267-178.json`]: { format: 1, routes: [loop] },
            [`${tiles}/9-268-178.json`]: { format: 1, routes: [] },
        });
        expect(await loadRouteCell('9-267-178')).toEqual([loop]);
        expect(await loadRouteCell('9-268-178')).toEqual([]);
        // These neighbours share only an edge with the bounds.
        for (const cell of ['9-266-178', '9-269-178', '9-267-177', '9-268-179']) expect(await loadRouteCell(cell)).toBeNull();
        expect(fetch).toHaveBeenCalledTimes(2);
    });

    it('treats a covered cell without a file as an error', async () => {
        const { loadRouteCell } = await catalog('https://tiles.example/routes/tiles/{cell}.json', {});
        await expect(loadRouteCell('9-268-178')).rejects.toThrow('404');
    });

    it('reads the cells of a regional release from its one region file', async () => {
        const url = 'https://planner.example/routes/freiburg.json';
        const long = { ...record(10, ['9-267-178', '9-268-178', '9-269-178']), stages: [21, 22] };
        const { fetch, loadRouteCell } = await catalog(url, { [url]: { format: 1, routes: [long, record(21, ['9-267-178']), record(22, ['9-268-178', '9-269-178'])] } });
        expect((await loadRouteCell('9-267-178'))?.map((route) => route.id)).toEqual([10, 21]);
        expect((await loadRouteCell('9-268-178'))?.map((route) => route.id)).toEqual([10, 22]);
        expect(await loadRouteCell('9-269-178')).toBeNull();
        expect(fetch).toHaveBeenCalledTimes(1);
    });
});
