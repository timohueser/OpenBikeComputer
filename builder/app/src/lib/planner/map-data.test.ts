import { afterEach, describe, expect, it, vi } from 'vitest';
import { validateStyleMin } from '@maplibre/maplibre-gl-style-spec';
import { testConfig } from '../../../test-support/planner/config';
import { plannerConfig } from './config';

afterEach(() => {
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
    vi.resetModules();
});

describe('planner config', () => {
    it('gives the planner map the basemap, assets, terrain and coverage of one release', async () => {
        const { mapStyle } = await import('./map-style');
        for (const theme of ['light', 'dark'] as const) {
            const style = mapStyle(theme, testConfig, 'dem://tiles', 'contours://tiles');
            expect(validateStyleMin(style)).toEqual([]);
            expect(style.sources.basemap).toHaveProperty('url', testConfig.basemap);
            expect(style.glyphs).toBe(testConfig.glyphs);
            expect(style.sprite).toBe(`${testConfig.sprites}/${theme}`);
            for (const source of ['terrain', 'contours']) {
                expect(style.sources[source]).toMatchObject({ bounds: testConfig.bounds, attribution: testConfig.terrain_attribution });
            }
        }
    });

    it('gives Leaflet maps the basemap alone, without places, terrain or cycleways', async () => {
        const { basemapStyle } = await import('./map-style');
        for (const theme of ['light', 'dark'] as const) {
            const style = basemapStyle(theme, testConfig);
            expect(validateStyleMin(style)).toEqual([]);
            expect(style.sprite).toMatch(new RegExp(`/${theme}$`));
            expect(Object.keys(style.sources)).toEqual(['basemap']);
            expect(style.layers.map((layer) => layer.id)).not.toContain('pois');
            expect(style.layers.some((layer) => layer.type === 'background')).toBe(true);
            expect(style.layers.every((layer) => !('source' in layer) || layer.source === 'basemap')).toBe(true);
        }
    });

    it('resolves the paths of a local preview against the page, and keeps template tokens', () => {
        const local = plannerConfig(JSON.stringify({ ...testConfig, basemap: 'pmtiles:///@fs/data/maps/basemap.pmtiles',
            terrain: '/tiles/terrain/{z}/{x}/{y}.webp', layers: { snow: '/@fs/data/maps/snow.pmtiles' } }), 'http://localhost:4175/planner.html');
        expect([local.basemap, local.terrain, local.layers.snow, local.search]).toEqual(['pmtiles://http://localhost:4175/@fs/data/maps/basemap.pmtiles',
            'http://localhost:4175/tiles/terrain/{z}/{x}/{y}.webp', 'http://localhost:4175/@fs/data/maps/snow.pmtiles', testConfig.search]);
    });

    it('refuses a config without a field that the planner reads', () => {
        expect(plannerConfig(JSON.stringify(testConfig))).toEqual(testConfig);
        const { layers: _, ...withoutLayers } = testConfig;
        for (const [config, field] of [[withoutLayers, 'layers'], [{ ...testConfig, terrain: 'tiles/{z}/{x}/{y}.webp' }, 'terrain'],
            [{ ...testConfig, bounds: [10.5, 47.5, 7.45, 49.85] }, 'bounds'], [{ ...testConfig, name: '' }, 'name']] as const) {
            expect(() => plannerConfig(JSON.stringify(config))).toThrow(`invalid fields: ${field}.`);
        }
        expect(() => plannerConfig(undefined)).toThrow('VITE_PLANNER_CONFIG');
    });
});

// Cell-aligned bounds of the cells 9-267-178 and 9-268-178, as the release builder writes them.
const BOUNDS = [7.734375, 47.51720069783939, 9.140625, 47.98992166741417];
const record = (id: number, cells: string[]) => ({ id, kind: 'hiking', name: `Route ${id}`, rank: 1, loop: false,
    length_m: 3000, ascent_m: 10, descent_m: 10, cells, line_udeg: [8000000, 47800000, 100, 100], via: [] });

async function catalog(url: string, files: Record<string, unknown>) {
    vi.stubEnv('VITE_PLANNER_CONFIG', JSON.stringify({ ...testConfig, routes: url, bounds: BOUNDS }));
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
