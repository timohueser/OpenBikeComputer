import { describe, expect, it } from 'vitest';
import { POINT_WORDS, routePoints } from './route';

describe('routePoints', () => {
    it('keeps the vertices, adds points at most 5 m apart, and spans 50 m to each side for the grade', () => {
        // About 222 m north, then a repeated vertex, then about 76 m east.
        const words = routePoints([[8.3, 46.8], [8.3, 46.802], [8.3, 46.802], [8.301, 46.802]]);
        const ints = new Int32Array(words.buffer), floats = new Float32Array(words.buffer), n = words.length / POINT_WORDS;
        const along = Array.from({ length: n }, (_, i) => floats[i * POINT_WORDS + 2]);
        const steps = along.slice(1).map((metres, i) => metres - along[i]);
        expect(Math.max(...steps)).toBeLessThanOrEqual(5);
        expect(Math.min(...steps)).toBeGreaterThan(4);
        expect(along.at(-1)).toBeCloseTo(222.4 + 76.1, 0);
        // The bend is a point of its own: the line leaves it east.
        const bend = along.findIndex(metres => metres > 222);
        expect(ints[bend * POINT_WORDS]).toBe(ints[0]);
        expect(ints[(bend + 1) * POINT_WORDS]).toBeGreaterThan(ints[0]);
        for (let i = 0; i < n; i++) {
            const [behind, ahead] = [words[i * POINT_WORDS + 3], words[i * POINT_WORDS + 4]];
            expect(along[i] - along[behind]).toBeLessThanOrEqual(50);
            expect(along[ahead] - along[i]).toBeLessThanOrEqual(50);
            if (behind > 0) expect(along[i] - along[behind - 1]).toBeGreaterThan(50);
            if (ahead < n - 1) expect(along[ahead + 1] - along[i]).toBeGreaterThan(50);
        }
    });
});
