import { describe, expect, it } from 'vitest';
import { Shadow } from './shadow';

/** The map point (x, y, depth) of a render point under a column-major matrix. */
const project = (m: Float32Array, [x, y, z]: number[]) => [0, 1, 2].map(r => m[r] * x + m[4 + r] * y + m[8 + r] * z + m[12 + r]);
const unit = (v: number[]) => v.map(x => x / Math.hypot(...v));
const NOTHING = { casters: false, reshaped: Infinity };
/** A sun `degrees` high from the south-west. */
const sunAt = (degrees: number) => {
    const a = degrees * Math.PI / 180;
    return [-Math.cos(a) * Math.SQRT1_2, -Math.cos(a) * Math.SQRT1_2, Math.sin(a)];
};

describe('the object shadow maps', () => {
    const sun = unit([0.5, -0.3, 0.4]);

    it('put a crown and the ground where the sun casts its shadow on one texel, the crown nearer the sun, from any eye and any centre', () => {
        const shadow = new Shadow(), crown = [1010, 2005, 520];
        const texel = (eye: number[]) => {
            const m = shadow.matrix(0, eye), at = crown.map((v, i) => v - eye[i]);
            const [a, b] = [project(m, at), project(m, at.map((v, i) => v - (20 / sun[2]) * sun[i]))];
            expect(Math.hypot(a[0] - b[0], a[1] - b[1]) * 1024).toBeLessThan(1e-3);
            expect(a[2]).toBeLessThan(b[2]);
            return a.slice(0, 2).map(v => (v * 1024) % 1);
        };
        expect(shadow.due([1000, 2000, 500], 1.6, sun, NOTHING, false)).toBe(0);
        expect(shadow.due([1000, 2000, 500], 1.6, sun, NOTHING, false)).toBe(1);
        const first = texel([1000, 2000, 501.6]);
        texel([1030, 1980, 520]);
        // Drawn again about another centre, the crown keeps its place in its texel.
        expect(shadow.due([1060, 2000, 503], 1.6, sun, NOTHING, false)).toBe(0);
        texel([1060, 2000, 504.6]).forEach((v, i) => {
            const d = Math.abs(v - first[i]) % 1;
            expect(Math.min(d, 1 - d)).toBeLessThan(1e-3);
        });
    });

    it('holds a receiver 100 m away and 100 m above or below the eye at a sun of 10°, on its caster\'s texel', () => {
        const shadow = new Shadow(), low = sunAt(10), eye = [0, 0, 1.6];
        expect(shadow.due([0, 0, 0], 1.6, low, NOTHING, false)).toBe(0);
        expect(shadow.due([0, 0, 0], 1.6, low, NOTHING, false)).toBe(1);
        for (const [x, z] of [[100, 100], [-100, 100], [100, -100], [-100, -100]]) {
            const m = shadow.matrix(1, eye), receiver = [x, 0, z].map((v, i) => v - eye[i]);
            const [a, b] = [project(m, receiver), project(m, receiver.map((v, i) => v + (15 / low[2]) * low[i]))];
            expect(Math.max(Math.abs(a[0]), Math.abs(a[1]))).toBeLessThan(0.8);
            expect(a[2]).toBeGreaterThan(0);
            expect(a[2]).toBeLessThan(1);
            expect(Math.hypot(a[0] - b[0], a[1] - b[1]) * 1024).toBeLessThan(1e-3);
        }
    });

    it('draws a map again when the eye leaves its centre or its height, the sun turns, or new casters waited long enough', () => {
        const shadow = new Shadow(), turned = [sun[1], sun[0], sun[2]], casters = { casters: true, reshaped: Infinity };
        expect(shadow.due([0, 0, 0], 1.6, sun, NOTHING, false)).toBe(0);
        expect(shadow.due([0, 0, 0], 1.6, sun, NOTHING, false)).toBe(1);
        expect(shadow.due([40, 0, 0], 1.6, sun, casters, true)).toBe(-1);
        expect(shadow.due([60, 0, 0], 1.6, sun, NOTHING, true)).toBe(0);
        expect(shadow.due([60, 0, 20], 1.6, sun, NOTHING, true)).toBe(0);
        // Both are due: the one that waited longer draws first.
        expect(shadow.due([60, 0, 20], 1.6, turned, NOTHING, false)).toBe(1);
        expect(shadow.due([60, 0, 20], 1.6, turned, NOTHING, false)).toBe(0);
        // While the eye moves, new casters wait for the cascade's frames; in a still view they draw at once.
        expect(shadow.due([60, 0, 20], 1.6, turned, casters, true)).toBe(-1);
        for (let frame = 0; frame < 28; frame++) expect(shadow.due([60, 0, 20], 1.6, turned, NOTHING, true)).toBe(-1);
        expect(shadow.due([60, 0, 20], 1.6, turned, NOTHING, true)).toBe(0);
        expect(shadow.due([60, 0, 20], 1.6, turned, casters, false)).toBe(1);
        expect(shadow.due([60, 0, 20], 1.6, turned, NOTHING, false)).toBe(0);
        expect(shadow.pending).toBe(false);
        // High above the ground only the far map draws; under a sun of 2.9° or less none, and none waits.
        expect(shadow.due([60, 0, 20], 500, sun, NOTHING, false)).toBe(1);
        expect(shadow.due([60, 0, 20], 1.6, sunAt(2.5), casters, false)).toBe(-1);
        expect(shadow.pending).toBe(false);
        expect(shadow.due([60, 0, 20], 1.6, sunAt(4), NOTHING, false)).toBe(0);
    });
});
