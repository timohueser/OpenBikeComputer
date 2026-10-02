import { describe, expect, it } from 'vitest';
import type { Coordinate } from '../editor';
import { searchLine } from './plan';

describe('search line', () => {
    it('drops points close to the kept line and keeps the full-line kilometres and hours', () => {
        // The middle point is about 110 m off the straight start-to-finish line; the others lie on the kept line.
        const line: Coordinate[] = [[8, 48], [8.01, 48.0005], [8.020004, 48.001004], [8.03, 48.0005], [8.04, 48]];
        expect(searchLine(line, [0, 1, 2, 3, 4], [0, 360, 720, 1080, 1440])).toEqual({
            coordinates: [[8, 48], [8.02, 48.001], [8.04, 48]], km: [0, 2, 4], hours: [0, .2, .4],
        });
    });
});
