import { describe, expect, it } from 'vitest';
import { POINT_WORDS, routePoints } from './route';

describe('routePoints', () => {
    it('keeps the vertices, adds points at most 5 m apart, and gives each the colour of the edge that ends at it', () => {
        // About 222 m north, then a repeated vertex, then about 76 m east.
        const words = routePoints([[8.3, 46.8], [8.3, 46.802], [8.3, 46.802], [8.301, 46.802]], ['#ff0000', '#00ff00', '#0000ff']);
        const ints = new Int32Array(words.buffer), n = words.length / POINT_WORDS;
        const colours = Array.from({ length: n }, (_, i) => words[i * POINT_WORDS + 2]);
        expect(n).toBe(1 + Math.ceil(222.4 / 5) + Math.ceil(76.1 / 5));
        // The bend is a point of its own and the last red one (red packs as 0x0000ff): the line leaves it east, in blue.
        const bend = colours.lastIndexOf(0x0000ff);
        expect(colours.slice(0, bend + 1).every(c => c === 0x0000ff)).toBe(true);
        expect(colours.slice(bend + 1).every(c => c === 0xff0000)).toBe(true);
        expect(ints[bend * POINT_WORDS]).toBe(ints[0]);
        expect(ints[(bend + 1) * POINT_WORDS]).toBeGreaterThan(ints[0]);
    });
});
