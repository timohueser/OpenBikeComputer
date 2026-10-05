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
        const local = plannerConfig({ ...testConfig, basemap: 'pmtiles:///@fs/data/maps/basemap.pmtiles',
            terrain: '/tiles/terrain/{z}/{x}/{y}.webp', layers: { snow: '/@fs/data/maps/snow.pmtiles' } }, 'http://localhost:4175/planner.html');
        expect([local.basemap, local.terrain, local.layers.snow, local.search]).toEqual(['pmtiles://http://localhost:4175/@fs/data/maps/basemap.pmtiles',
            'http://localhost:4175/tiles/terrain/{z}/{x}/{y}.webp', 'http://localhost:4175/@fs/data/maps/snow.pmtiles', testConfig.search]);
    });

    it('refuses a config without a field that the planner reads', () => {
        expect(plannerConfig(testConfig)).toEqual(testConfig);
        const { layers: _, ...withoutLayers } = testConfig;
        for (const [config, field] of [[withoutLayers, 'layers'], [{ ...testConfig, terrain: 'tiles/{z}/{x}/{y}.webp' }, 'terrain'],
            [{ ...testConfig, bounds: [10.5, 47.5, 7.45, 49.85] }, 'bounds'], [{ ...testConfig, name: '' }, 'name']] as const) {
            expect(() => plannerConfig(config)).toThrow(`invalid fields: ${field}.`);
        }
        expect(() => plannerConfig(undefined)).toThrow('not an object');
    });
});

const CATALOG = 'https://maps.openbikecomputer.com/planner/catalog.json';
const release = (id: string) => ({ ...testConfig, id, routing: `https://api.test/releases/${id}/routing` });

/** The planner modules of a page with no preview config, on a stub catalogue. */
async function page(catalogs: unknown[], objects: (url: string) => Response = () => new Response('{}')) {
    vi.stubEnv('VITE_PLANNER_CONFIG', '');
    const reload = vi.fn();
    vi.stubGlobal('location', { href: 'https://openbikecomputer.com/plan/', reload });
    const fetch = vi.fn(async (url: string) => url === CATALOG ? Response.json(catalogs.length > 1 ? catalogs.shift() : catalogs[0]) : objects(url));
    vi.stubGlobal('fetch', fetch);
    return { fetch, reload, load: () => import('./map-data') };
}

describe('live catalogue', () => {
    it('gives the page the active release of the catalogue', async () => {
        const { load } = await page([{ format: 1, active: release('a'), previous: null }]);
        expect((await load()).config).toEqual(release('a'));
    });

    it('refuses a catalogue format that it does not know', async () => {
        const { load } = await page([{ format: 2, active: release('a') }]);
        await expect(load()).rejects.toThrow('format that this page does not know');
    });

    it('reads the catalogue again once after a 404 or no answer, and loads the page again on a new release', async () => {
        const { fetch, reload, load } = await page([{ format: 1, active: release('a') }, { format: 1, active: release('b') }],
            url => { if (url.endsWith('/route')) throw new TypeError('Failed to fetch'); return new Response('Not found', { status: 404 }); });
        await load();
        const { releaseFetch } = await import('./release');
        await expect(releaseFetch('https://api.test/releases/a/routing/v1/route')).rejects.toThrow(TypeError);
        expect((await releaseFetch('https://api.test/releases/a/routing/v1/region')).status).toBe(404);
        await vi.waitFor(() => expect(reload).toHaveBeenCalledOnce());
        expect(fetch.mock.calls.filter(([url]) => url === CATALOG)).toHaveLength(2);
    });

    it('loads the MapLibre objects of the release through the release fetch', async () => {
        const tilejson = { tiles: ['https://tiles.test/releases/a/basemap/{z}/{x}/{y}.mvt'] };
        const { fetch, load } = await page([{ format: 1, active: release('a') }], () => Response.json(tilejson));
        await load();
        const { releaseProtocol, releaseUrl } = await import('./release');
        const url = releaseUrl('https://tiles.test/releases/a/basemap.json');
        expect(url).toBe('release://tiles.test/releases/a/basemap.json');
        expect((await releaseProtocol({ url, type: 'json' }, new AbortController())).data)
            .toEqual({ tiles: ['release://tiles.test/releases/a/basemap/{z}/{x}/{y}.mvt'] });
        expect(fetch).toHaveBeenLastCalledWith('https://tiles.test/releases/a/basemap.json', expect.anything());
    });

    it('keeps the page when the active release did not change, or when the plan is not saved', async () => {
        for (const [catalogs, saved] of [[[release('a')], true], [[release('a'), release('b')], false]] as const) {
            vi.resetModules();
            const { fetch, reload, load } = await page(catalogs.map(active => ({ format: 1, active })), () => new Response('', { status: 404 }));
            await load();
            const { beforeReload, releaseFetch } = await import('./release');
            const wait = vi.fn(async () => saved);
            beforeReload(wait);
            await releaseFetch('https://api.test/releases/a/routing/v1/region');
            await vi.waitFor(() => expect(fetch.mock.calls.filter(([url]) => url === CATALOG)).toHaveLength(2));
            await new Promise(resolve => setTimeout(resolve));
            expect(reload).not.toHaveBeenCalled();
            expect(wait).toHaveBeenCalledTimes(saved ? 0 : 1);
        }
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
