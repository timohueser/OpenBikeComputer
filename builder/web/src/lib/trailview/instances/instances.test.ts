import { describe, expect, it } from 'vitest';
import { tree } from '../models/library';
import { outlines } from './instances';

describe('the impostor outlines', () => {
    it('hold the mesh seen along every frame and its neighbours', () => {
        const mesh = tree('norway-spruce', 'summer', 1), p = mesh.position;
        let radius = 0;
        for (let i = 0; i < p.length; i += 3) radius = Math.max(radius, Math.hypot(p[i], p[i + 1], p[i + 2] - 0.5));
        const out = outlines(mesh, radius);
        // The most that a vertex lies outside an outline, over every frame and neighbour.
        let outside = -Infinity;
        for (let frame = 0; frame < 64; frame++) {
            const [low, high, w0, w1] = out.slice(4 * frame, 4 * frame + 4);
            // The frame itself and its eight neighbours in the grid, as the dither of impostor.wgsl picks them.
            const x = frame % 8, y = Math.floor(frame / 8), nears: number[] = [];
            for (let j = Math.max(0, y - 1); j <= Math.min(7, y + 1); j++) for (let i = Math.max(0, x - 1); i <= Math.min(7, x + 1); i++) nears.push(8 * j + i);
            for (const near of nears) {
                const ex = ((near % 8) + 0.5) / 4 - 1, ey = (Math.floor(near / 8) + 0.5) / 4 - 1, px = (ex + ey) / 2, py = (ex - ey) / 2;
                const pz = 1 - Math.abs(px) - Math.abs(py), n = Math.hypot(px, py, pz), d = [px / n, py / n, pz / n];
                const flat = Math.hypot(d[0], d[1]), right = Math.abs(d[2]) < 0.999 ? [-d[1] / flat, d[0] / flat, 0] : [1, 0, 0];
                const up = [d[1] * right[2] - d[2] * right[1], d[2] * right[0] - d[0] * right[2], d[0] * right[1] - d[1] * right[0]];
                for (let i = 0; i < p.length; i += 3) {
                    const v = [p[i], p[i + 1], p[i + 2] - 0.5];
                    const x = Math.abs(v[0] * right[0] + v[1] * right[1] + v[2] * right[2]) / radius, y = (v[0] * up[0] + v[1] * up[1] + v[2] * up[2]) / radius;
                    outside = Math.max(outside, low - y, y - high, x - w0 - ((w1 - w0) * (y - low)) / (high - low));
                }
            }
        }
        expect(outside).toBeLessThan(1e-5);
        // A spruce is narrow at its top: a side view's outline is a trapezoid, not the frame's square.
        expect(out[3]).toBeLessThan(0.5 * out[2]);
    });
});
