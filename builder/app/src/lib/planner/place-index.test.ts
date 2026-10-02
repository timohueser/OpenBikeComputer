import { afterEach, describe, expect, it, vi } from 'vitest';
import { corridorTiles, nearRoute } from './place-index';
import type { Coordinate } from './editor';

afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); });

describe('hosted corridor places', () => {
    it('reads TileJSON and XYZ tiles without downloading an archive', async () => {
        const fetch = vi.fn().mockResolvedValueOnce(new Response(JSON.stringify({ maxzoom: 11, tiles: ['https://tiles.example/places/{z}/{x}/{y}.mvt'] })))
            .mockResolvedValueOnce(new Response(null, { status: 204 }));
        vi.stubGlobal('fetch', fetch);
        const { corridorPlaces } = await import('./place-index');
        expect(await corridorPlaces('https://tiles.example/places.json', [[7.589, 47.557]], .5)).toEqual([]);
        expect(fetch.mock.calls.map(call => call[0])).toEqual(['https://tiles.example/places.json', 'https://tiles.example/places/11/1067/715.mvt']);
    });
});

// Basel lies in z12 tile 2134/1431, which spans 7.559–7.646° E and 47.517–47.577° N.
describe('corridor tiles', () => {
    it('adds the neighbouring tiles a buffer reaches into', () => {
        expect(corridorTiles([[7.589, 47.557]], .5, 12)).toEqual(['12/2134/1431']);
        expect(corridorTiles([[7.589, 47.557]], 3, 12).sort()).toEqual(['12/2133/1430', '12/2133/1431', '12/2134/1430', '12/2134/1431']);
    });

    it('covers every tile along a line without gaps', () => {
        expect(corridorTiles([[7.4, 47.55], [7.8, 47.55]], 1, 12).sort()).toEqual([2132, 2133, 2134, 2135, 2136].map(x => `12/${x}/1431`));
    });
});

// At 48° N, one kilometre is 0.00904° of latitude and 0.01343° of longitude.
describe('route distance', () => {
    const line = Array.from({ length: 100 }, (_, i): Coordinate => [8 + i / 99, 48]);
    const near = nearRoute(line, 5);

    it('keeps a place beside any part of the route and drops one beyond the corridor', () => {
        expect(near([8.9, 48 + 4 * .00904])).toBe(true);
        expect(near([8.9, 48 + 6 * .00904])).toBe(false);
    });

    it('measures past the route end to the end point', () => {
        expect(near([9 + 4 * .01343, 48])).toBe(true);
        expect(near([9 + 6 * .01343, 48])).toBe(false);
    });
});
