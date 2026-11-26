import { afterEach, describe, expect, it, vi } from 'vitest';
import { PbfWriter } from 'pbf';
import membership from '../../../../../planner/search/place-kinds.json';
import presentation from './poi-kinds.json';
import { corridorTiles, placeSource, poiPlace } from './place-index';
import { kmPerDegree } from './geo';

afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); });

describe('place identities', () => {
    it('gives every rider-place kind one category and a presentation label', () => {
        const kinds = Object.values(membership).flat();
        expect(new Set(kinds).size).toBe(kinds.length);
        expect(Object.keys(presentation.categories).sort()).toEqual(Object.keys(membership).sort());
        for (const kind of presentation.on) expect(Object.keys(membership)).toContain(kind);
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
        expect(await corridorPlaces('https://tiles.example/places.json', [[7.589, 47.557]], 'food')).toEqual([]);
        expect(fetch.mock.calls.map(call => call[0])).toEqual(['https://tiles.example/places.json', 'https://tiles.example/places/11/1067/715.mvt']);
    });

    it('keeps the places of a category within its distance from the route, with that distance', async () => {
        // A restaurant and a tap 400 m north of the route, a restaurant at 700 m: food reaches 500 m, water 250 m.
        const north = (metres: number) => 47.557 + metres / 1000 / kmPerDegree;
        const tile = placeTile([['restaurant', north(400)], ['drinking_water', north(400)], ['restaurant', north(700)]]);
        vi.stubGlobal('fetch', vi.fn(async (url: string) => url.endsWith('.json')
            ? new Response(JSON.stringify({ maxzoom: 11, tiles: ['https://tiles.example/places/{z}/{x}/{y}.mvt'] })) : new Response(new Uint8Array(tile))));
        const { corridorPlaces } = await import('./place-index');
        const route: [number, number][] = [[7.58, 47.557], [7.6, 47.557]];
        const food = await corridorPlaces('https://tiles.example/places.json', route, 'food');
        expect(food.map(({ place }) => place.id)).toEqual(['poi-1']);
        expect(food[0].offM).toBeCloseTo(400, 0);
        expect(await corridorPlaces('https://tiles.example/places.json', route, 'water')).toEqual([]);
    });

    it('gives the keys of the place tiles that a view touches', async () => {
        vi.stubGlobal('fetch', vi.fn().mockResolvedValueOnce(new Response(JSON.stringify({ maxzoom: 11, tiles: ['https://tiles.example/places/{z}/{x}/{y}.mvt'] }))));
        const { placeTileKeys } = await import('./place-index');
        // Lucerne, across the border of two columns at 8.4375° E and of two rows at 47.04° N.
        expect(await placeTileKeys('https://tiles.example/places.json', [8.3, 47.0, 8.5, 47.1])).toEqual(['11/1071/719', '11/1071/720', '11/1072/719', '11/1072/720']);
    });
});

/** A vector tile with a layer `pois`: a point for each place, with its kind and its coordinate at 7.59° E. */
function placeTile(places: [string, number][]) {
    const pbf = new PbfWriter(), keys = ['kind', 'lon', 'lat'];
    pbf.writeMessage(3, (_, layer) => {
        layer.writeVarintField(15, 2);
        layer.writeStringField(1, 'pois');
        places.forEach((_, i) => layer.writeMessage(2, (__, feature) => {
            feature.writeVarintField(1, i + 1);
            feature.writePackedVarint(2, keys.flatMap((___, k) => [k, i * keys.length + k]));
            feature.writeVarintField(3, 1);
            feature.writePackedVarint(4, [9, 4096, 4096]);
        }, undefined));
        for (const key of keys) layer.writeStringField(3, key);
        for (const [kind, lat] of places) {
            layer.writeMessage(4, (__, value) => value.writeStringField(1, kind), undefined);
            for (const number of [7.59, lat]) layer.writeMessage(4, (__, value) => value.writeDoubleField(3, number), undefined);
        }
        layer.writeVarintField(5, 4096);
    }, undefined);
    return pbf.finish();
}

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
