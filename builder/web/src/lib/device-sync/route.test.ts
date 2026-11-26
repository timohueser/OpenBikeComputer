import { describe, expect, it } from 'vitest';
import { cumulative, type Coordinate } from '../planner/geo';
import { testLine, testTrip } from '../../../test-support/planner/trip';
import { cut } from './route';

describe('cut', () => {
    it('ends a day at the nearest point of the line within 1 mm', () => {
        // Points 1 and 2 are about 0.5 mm apart; the day end is 0.1 mm before point 2.
        const coordinates: Coordinate[] = [[8, 47], [8, 47.001], [8, 47.001 + 4.5e-9], [8, 47.002]];
        const line = { ...testLine(testTrip()), coordinates, elevation: [400, 410, 410, 420], edges: {} }, lengths = cumulative(coordinates);
        expect(lengths[2] - lengths[1]).toBeLessThan(1e-6);
        expect(cut(line, 0, lengths[2] - 1e-7).coordinates).toEqual(coordinates.slice(0, 3));
    });
});
