import { describe, expect, it } from 'vitest';
import type { Coordinate } from '../editor';
import { searchLine } from './plan';

describe('search line', () => {
    it('drops points close to the kept line and keeps the full-line kilometres and hours', () => {
        // The middle point is about 110 m off the straight start-to-finish line; the others lie on the kept line.
        const line: Coordinate[] = [[8, 48], [8.01, 48.0005], [8.020004, 48.001004], [8.03, 48.0005], [8.04, 48]];
        expect(searchLine(line, [0, 1, 2.0004, 3, 4.0006], [0, 360, 720, 1080, 1440])).toEqual({
            // The last kilometre stays exact, so the last day still ends at the route length.
            coordinates: [[8, 48], [8.02, 48.001], [8.04, 48]], km: [0, 2, 4.0006], hours: [0, .2, .4],
        });
    });
    it('keeps the point where the pace changes on a straight road', () => {
        // 3 km of climbing at 8 km/h, then 3 km of descent at 35 km/h.
        const line: Coordinate[] = Array.from({ length: 7 }, (_, i) => [8 + i * .0134, 48]);
        const seconds = [0, 450, 900, 1350, 1453, 1556, 1659];
        expect(searchLine(line, [0, 1, 2, 3, 4, 5, 6], seconds).km).toEqual([0, 3, 6]);
    });
});
