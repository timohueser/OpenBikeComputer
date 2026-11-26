import { describe, expect, it } from 'vitest';
import { startOf } from './start';

describe('startOf', () => {
    it('points from the first point along a straight line', () => {
        expect(startOf([8, 46, 8, 46.01])).toEqual({ at: [8, 46], bearing: 0 });
        expect(startOf([8, 46, 8.01, 46])?.bearing).toBeCloseTo(90);
    });

    // A first segment of 8 m to the east would turn the arrow across the route, which goes north.
    it('looks past a short first segment', () => {
        expect(startOf([8, 46, 8.0001, 46, 8.0001, 46.01])?.bearing).toBeLessThan(1);
    });

    it('gives a loop the direction it is ridden in, not the way back to its end', () => {
        expect(startOf([8, 46, 8.01, 46, 8.01, 46.01, 8, 46.01, 8, 46])?.bearing).toBeCloseTo(90);
    });

    it('takes the last point of a line shorter than the look ahead, and needs two points', () => {
        expect(startOf([8, 46, 8.0001, 46, 8, 45.9995])?.bearing).toBeCloseTo(180);
        expect(startOf([8, 46])).toBeUndefined();
        expect(startOf([])).toBeUndefined();
    });
});
