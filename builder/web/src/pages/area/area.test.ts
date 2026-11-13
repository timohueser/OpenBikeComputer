import { describe, expect, it } from 'vitest';
import { CatalogClient } from '../../lib/catalog/client';
import { EXAMPLE_ROOT, exampleCatalog, fixtureIndices } from '../../lib/catalog/testdata';
import { degreesToUbox } from '../../lib/coverage/shape';
import { CoverageStore } from '../../lib/coverage/store.svelte';
import { distanceM } from '../../lib/library/summary';
import { deviceArea, sameArea, sameMap, type MapArea } from './area';
import { routesOutside } from './outside';

const regions = exampleCatalog.regions;
const box = { minLat: 46_000_000, minLon: 8_000_000, maxLat: 46_500_000, maxLon: 9_000_000 };
const sent: MapArea = { regions: ['europe/switzerland/basel-stadt'], boxes: [box], light: 'default', dark: 'custom-night' };

describe('deviceArea', () => {
    it('is the area of the map that this browser sent, while that map is on the card', () => {
        expect(deviceArea({ map: 'Alps', mapObject: '7:2', built: { object: '7:2', ...sent } }, regions)).toEqual(sent);
        expect(deviceArea({ map: 'Alps', mapObject: '7:3', built: { object: '7:2', ...sent } }, regions)).toBeNull();
    });

    it('is the catalog region with the name of the map, in the first colours of the catalog', () => {
        expect(deviceArea({ map: 'Switzerland', mapObject: '7:1', built: null }, regions))
            .toEqual({ regions: ['europe/switzerland'], boxes: [], light: 'default', dark: 'dusk' });
    });

    it('is not known for another map, for no map and for no device', () => {
        expect(deviceArea({ map: 'Alps', mapObject: '7:1', built: null }, regions)).toBeNull();
        expect(deviceArea({ map: null, mapObject: null, built: null }, regions)).toBeNull();
        expect(deviceArea(null, regions)).toBeNull();
    });
});

describe('sameMap', () => {
    const map: MapArea = { regions: ['a', 'b'], boxes: [box, { ...box, maxLon: 9_500_000 }], light: 'default', dark: 'dusk' };

    it('takes the regions and the boxes in any order', () => {
        expect(sameMap(map, { ...map, regions: ['b', 'a'], boxes: [...map.boxes].reverse() })).toBe(true);
        expect(sameMap(map, { ...map, regions: ['a'] })).toBe(false);
        expect(sameMap(map, { ...map, boxes: [box, box] })).toBe(false);
    });

    it('sees other colours as the same area and another map', () => {
        expect(sameArea(map, { ...map, dark: 'custom-night' })).toBe(true);
        expect(sameMap(map, { ...map, dark: 'custom-night' })).toBe(false);
    });
});

describe('routesOutside', () => {
    /** The area is a box in the detail cell 18/1204/1052. The catalog has map data in the next cell to the east, and
     * none in the cell after it. */
    function boxStore(box = degreesToUbox(47.2, 7.35, 47.4, 7.6)): CoverageStore {
        const offline = (async () => new Response('offline', { status: 503 })) as unknown as typeof fetch;
        const store = new CoverageStore(CatalogClient.fromBody(EXAMPLE_ROOT, 'https://maps.example.org/catalog/catalog.json', { fetchImpl: offline }), EXAMPLE_ROOT, null);
        store.indices = fixtureIndices(exampleCatalog, {
            fine: [{ id: '18/1204/1052', bytes: 552 }, { id: '18/1204/1053', bytes: 424 }],
            network: [{ id: '18/1204/1052', bytes: 296 }, { id: '18/1204/1053', bytes: 168 }],
        }, { fine: [{ start: '18/1204/1055', end: '18/1204/1055' }] });
        store.addBox(box);
        return store;
    }
    const routes = [
        { id: 'leaves', line: [7.5, 47.3, 7.58, 47.3, 7.65, 47.3, 7.7, 47.3] },
        { id: 'inside', line: [7.4, 47.3, 7.55, 47.3] },
        { id: 'no-data', line: [7.5, 47.3, 7.9, 47.3] },
        { id: 'one-point', line: [7.7, 47.3] },
    ];

    it('gives the length outside the area, the box around it and the region there, only where the catalog has map data', () => {
        const store = boxStore(), [leaves, ...rest] = routesOutside(store, routes);
        expect(rest).toEqual([]);
        expect(leaves.id).toBe('leaves');
        expect(leaves.metres).toBeCloseTo(distanceM(7.58, 47.3, 7.65, 47.3) / 2 + distanceM(7.65, 47.3, 7.7, 47.3), 3);
        // About 1 km of room on each side.
        expect(leaves.boxes).toEqual([degreesToUbox(47.3 - 0.01, 7.58 - 0.01, 47.3 + 0.01, 7.7 + 0.01)]);
        expect(leaves.region?.id).toBe('europe/switzerland');
    });

    it('finds no route outside after the box around it is in the area', () => {
        const store = boxStore();
        store.addBox(routesOutside(store, routes)[0].boxes[0]);
        expect(routesOutside(store, routes)).toEqual([]);
    });

    it('gives one box for each side on which a route leaves the area', () => {
        // The area is the middle cell; the route comes from the cell to the west and goes on to the east.
        const store = boxStore(degreesToUbox(47.2, 7.65, 47.4, 7.85));
        const across = [{ id: 'across', line: [7.5, 47.3, 7.58, 47.3, 7.65, 47.3, 7.75, 47.3, 7.85, 47.3, 8.0, 47.3, 8.2, 47.3, 8.3, 47.3] }];
        const [{ boxes }] = routesOutside(store, across);
        expect(boxes).toEqual([degreesToUbox(47.29, 7.5 - 0.01, 47.31, 7.65 + 0.01), degreesToUbox(47.29, 8.0 - 0.01, 47.31, 8.3 + 0.01)]);
        boxes.forEach(box => store.addBox(box));
        expect(routesOutside(store, across)).toEqual([]);
    });
});
