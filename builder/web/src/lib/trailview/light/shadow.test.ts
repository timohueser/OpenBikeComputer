import { describe, expect, it } from 'vitest';
import { Shadow } from './shadow';

/** The map point (x, y, depth) of a render point under a column-major matrix. */
const project = (m: Float32Array, [x, y, z]: number[]) => [0, 1, 2].map(r => m[r] * x + m[4 + r] * y + m[8 + r] * z + m[12 + r]);

describe('the object shadow map', () => {
    const sun = [0.5, -0.3, 0.4].map(v => v / Math.hypot(0.5, -0.3, 0.4));

    it('puts a crown and the ground where the sun casts its shadow on one texel, the crown nearer the sun, from any eye', () => {
        const shadow = new Shadow();
        expect(shadow.due([1000, 2000, 500], 1.6, sun, false, false)).toBe(true);
        for (const eye of [[1000, 2000, 501.6], [1030, 1980, 520]]) {
            const m = shadow.matrix(eye), crown = [1010 - eye[0], 2005 - eye[1], 520 - eye[2]];
            const ground = crown.map((v, i) => v - (20 / sun[2]) * sun[i]);
            const [a, b] = [project(m, crown), project(m, ground)];
            expect(Math.hypot(a[0] - b[0], a[1] - b[1]) * 1024).toBeLessThan(1e-3);
            expect(a[2]).toBeLessThan(b[2]);
        }
    });

    it('draws again when the eye leaves the centre or the sun turns, and new content waits while the camera moves', () => {
        const shadow = new Shadow();
        shadow.due([0, 0, 0], 1.6, sun, false, false);
        expect(shadow.due([40, 0, 0], 1.6, sun, true, true)).toBe(false);
        expect(shadow.due([60, 0, 0], 1.6, sun, false, true)).toBe(true);
        expect(shadow.due([60, 0, 0], 1.6, [sun[1], sun[0], sun[2]], false, false)).toBe(true);
        expect(shadow.due([60, 0, 0], 1.6, [sun[1], sun[0], sun[2]], true, false)).toBe(true);
        expect(shadow.due([60, 0, 0], 300, sun, false, false)).toBe(false);
    });
});
