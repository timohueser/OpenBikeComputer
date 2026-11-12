import { describe, expect, it } from 'vitest';
import type { Summary } from '../library/records';
import { along, distanceAt, nearest, pack, runs } from './line';

// Nine points 100 m apart on the equator. In the image they run along y = 0; point 6 has no place.
const x = [0, 100, 110, 210, 220, 320, NaN, 400, 500];
const visible = [1, 1, 0, 1, 0, 0, 0, 1, 1];
const points = pack({
    x: Float32Array.from(x), y: Float32Array.from(x, value => Number.isNaN(value) ? NaN : 0),
    distance: Float32Array.from(x, (value, i) => Number.isNaN(value) ? NaN : 1000 * (i + 1)), visible: Uint8Array.from(visible),
});
const summary = {
    line: x.flatMap((_, i) => [i * 0.001, 0]), heights: x.map(() => null), bounds: [0, 0, 0.008, 0],
    lengthM: 800, climbM: 0, highestM: null, seconds: 0, bike: 'road',
} satisfies Summary;

describe('the line of a cover', () => {
    it('keeps the distance from the eye to 5 %', () => {
        for (const i of [0, 4, 8]) expect(Math.abs(distanceAt(points, i) / (1000 * (i + 1)) - 1)).toBeLessThan(0.05);
        expect(distanceAt(points, 6)).toBeNaN();
    });

    it('joins a stretch of less than 16 pixels to the stretch before it, and has a gap at a point with no place', () => {
        // The hidden 10 px at point 2 joins the visible stretch; the hidden 10 px at point 4 and the hidden 100 px after it make 110 px.
        expect(runs(points)).toEqual([
            { from: 0, to: 3, visible: true },
            { from: 3, to: 5, visible: false },
            { from: 7, to: 8, visible: true },
        ]);
    });

    it('places the mark between two points, and none in the gap', () => {
        expect(along(points, summary, 50)?.x).toBeCloseTo(50);
        expect(along(points, summary, 150)?.x).toBeCloseTo(105);
        expect(along(points, summary, 650)).toBeUndefined();
    });

    it('gives the metres along the route of the point nearest to the pointer', () => {
        expect(nearest(points, summary, 105, 3)).toBeCloseTo(150);
        expect(nearest(points, summary, 450, -20)).toBeCloseTo(750);
    });

    it('keeps a segment to a point far beside the eye on its line', () => {
        // From the middle of the image toward a point 10 million pixels away, down and to the right at 1 : 5.
        const far = pack({ x: Float32Array.of(960, 960 + 1e7), y: Float32Array.of(640, 640 + 2e6), distance: Float32Array.of(5000, 10), visible: Uint8Array.of(1, 0) });
        const at = along(far, { ...summary, line: [0, 0, 0.001, 0], heights: [null, null], lengthM: 1e7 }, 1000);
        expect(at!.x).toBeCloseTo(1960, 0);
        expect(at!.y).toBeCloseTo(840, 0);
    });
});

