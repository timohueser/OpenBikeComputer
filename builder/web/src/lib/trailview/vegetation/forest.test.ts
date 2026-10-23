import { describe, expect, it } from 'vitest';
import { densest } from './canopy';
import { bluePoints } from './forest';

/** The least distance between the first n points, across the edges of the unit square. */
function spacing(points: Float32Array, n: number): number {
    let least = Infinity;
    for (let i = 0; i < n; i++) for (let j = 0; j < i; j++) {
        const dx = Math.abs(points[2 * i] - points[2 * j]), dy = Math.abs(points[2 * i + 1] - points[2 * j + 1]);
        least = Math.min(least, Math.hypot(Math.min(dx, 1 - dx), Math.min(dy, 1 - dy)));
    }
    return least;
}

describe('the forest', () => {
    it('places progressive blue noise: every first n points keep apart as an even spread of n would', () => {
        const points = bluePoints(512);
        expect(points.every(v => v >= 0 && v < 1)).toBe(true);
        // An even grid of n points is 1 / √n apart; white noise of 512 points comes far closer.
        for (const n of [8, 32, 128, 512]) expect(spacing(points, n) * Math.sqrt(n)).toBeGreaterThan(0.45);
    });

    it('bounds the density of a canopy cell by the levels above it', () => {
        const codes = new Uint16Array(256 * 256);
        // Height 20 m, cover 90 %, stems class 3; height 6 m, cover 40 %, stems class 6; open land; unknown.
        codes.set([40 | (9 << 7) | (3 << 11) | (2 << 14), 12 | (4 << 7) | (6 << 11), 0, 0xffff]);
        const levels = densest(codes);
        expect(levels.map(level => level.length)).toEqual([256, 128, 64, 32, 16, 8, 4, 2, 1].map(side => side * side));
        expect(levels[1][0]).toBe(12 | (9 << 7) | (6 << 11));
        expect(levels[8][0]).toBe(12 | (9 << 7) | (6 << 11));
        expect(levels[1][1]).toBe(0);
    });
});
