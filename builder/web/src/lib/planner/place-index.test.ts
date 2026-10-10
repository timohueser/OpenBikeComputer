import { afterEach, describe, expect, it, vi } from 'vitest';
import membership from '../../../../../planner/search/place-kinds.json';
import presentation from './poi-kinds.json';
import { corridorTiles, placeSource, poiPlace } from './place-index';

afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); });

describe('place identities', () => {
    it('gives every rider-place kind one category and a presentation label', () => {
        const kinds = Object.values(membership).flat();
        expect(new Set(kinds).size).toBe(kinds.length);
        expect(Object.keys(presentation.categories).sort()).toEqual(Object.keys(membership).sort());
        expect(Object.keys(presentation.labels).sort()).toEqual(kinds.sort());
        for (const [category, values] of Object.entries(membership)) {
            for (const kind of values) expect(poiPlace('n1', kind, undefined, [8, 48])).toMatchObject({ category,
                label: presentation.labels[kind as keyof typeof presentation.labels] });
        }
    });
    it('keeps OSM elements and curated Wikidata identities distinct for detail lookup', () => {
        for (const [type, letter] of [[1,'n'],[2,'w'],[3,'r'],[4,'Q']] as const) {
            const id = type * 2 ** 44 + 123;
            expect(placeSource(id)).toBe(`${letter}123`);
            expect(poiPlace(id, 'campsite', 'Camp', [8,48])?.id).toBe(`${letter}123`);
        }
        expect(placeSource('n123')).toBe('n123');
        expect(placeSource(123)).toBeUndefined();
        expect(placeSource(undefined)).toBeUndefined();
    });
});
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
