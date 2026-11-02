import { describe, expect, it } from 'vitest';
import { guidepost } from './amenities';

describe('the guidepost', () => {
    it('turns every face outward', () => {
        const { points } = guidepost(), count = points.length / 9;
        // Its parts are convex: triangles that share a corner are one part, and each face looks away from its part's middle.
        const part = Array.from({ length: count }, (_, t) => t), seen = new Map<string, number>();
        const root = (t: number): number => (part[t] === t ? t : (part[t] = root(part[t])));
        for (let i = 0; i < points.length; i += 3) {
            const key = points.slice(i, i + 3).join(), t = Math.floor(i / 9), other = seen.get(key);
            if (other === undefined) seen.set(key, t);
            else part[root(t)] = root(other);
        }
        const middles = new Map<number, number[]>();
        for (let i = 0; i < points.length; i += 3) {
            const r = root(Math.floor(i / 9)), sum = middles.get(r) ?? [0, 0, 0, 0];
            for (let k = 0; k < 3; k++) sum[k] += points[i + k];
            sum[3]++;
            middles.set(r, sum);
        }
        expect(middles.size).toBe(3);
        for (let t = 0; t < count; t++) {
            const [ax, ay, az, bx, by, bz, cx, cy, cz] = points.slice(9 * t, 9 * t + 9), u = [bx - ax, by - ay, bz - az], w = [cx - ax, cy - ay, cz - az];
            const n = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]], [sx, sy, sz, k] = middles.get(root(t))!;
            expect(n[0] * (ax + bx + cx - (3 * sx) / k) + n[1] * (ay + by + cy - (3 * sy) / k) + n[2] * (az + bz + cz - (3 * sz) / k)).toBeGreaterThan(0);
        }
    });
});
