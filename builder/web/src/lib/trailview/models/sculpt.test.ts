import { describe, expect, it } from 'vitest';
import { alongY, mesh, Sculpt } from './sculpt';

describe('the modelling kit', () => {
    it('turns every face of a skin away from its axis, and keeps each facet as one normal with its corners once', () => {
        const s = new Sculpt().skin(alongY([[-1, 0, 0.3, 0.4, 0.2], [0, 0, 0.5, 0.6, 0.3], [1, 0, 0.2, 0.2, 0.2]]), 8, 0x808080);
        const { position: p, normal: n, index } = mesh(s.model);
        for (let t = 0; t < index.length; t += 3) {
            const [a, b, c] = [index[t], index[t + 1], index[t + 2]];
            const mid = [0, 1, 2].map(d => (p[3 * a + d] + p[3 * b + d] + p[3 * c + d]) / 3);
            // Away from the y axis on the sides, along it at the caps.
            const out = Math.abs(mid[1]) > 0.99 ? [0, mid[1], 0] : [mid[0], 0, mid[2]];
            expect(n[3 * a] * out[0] + n[3 * a + 1] * out[1] + n[3 * a + 2] * out[2]).toBeGreaterThan(0);
        }
        // 2 bands of 8 quads with 4 corners each, and 2 caps of 8 corners.
        expect(p.length / 3).toBe(2 * 8 * 4 + 2 * 8);
        expect(index.length / 3).toBe(2 * 8 * 2 + 2 * 6);
    });

    it('builds the far tier with fewer sides and without the fine details', () => {
        const [near, far] = [0, 1].map(tier => {
            const s = new Sculpt(tier).skin(alongY([[-1, 0, 0.3, 0.3, 0.3], [1, 0, 0.3, 0.3, 0.3]]), 10, 0);
            const body = s.model.triangles;
            s.fine(() => s.box([0, 0, 0], [0.1, 0.1, 0.1], 0));
            return { body, details: s.model.triangles - body };
        });
        expect(far.body).toBeLessThan(near.body);
        expect([near.details, far.details]).toEqual([12, 0]);
    });
});
