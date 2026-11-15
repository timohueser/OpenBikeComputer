import { describe, expect, it } from 'vitest';
import { exampleCatalog } from '../catalog/testdata';
import { deviceArea, sameArea, sameMap, type MapArea } from './area';

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
