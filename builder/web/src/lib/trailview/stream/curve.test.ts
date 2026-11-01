import { describe, expect, it } from 'vitest';
import { curveOf, ROAD_SAG } from './curve';
import type { FeaturePart } from './features';

const part = (xy: number[], tangent: number[], z: number[], grade: number[]): FeaturePart => ({
    outer: false, xy: Float32Array.from(xy), z: Float32Array.from(z),
    plan: { along: 0, backward: false, tangent: Float32Array.from(tangent), grade: Float32Array.from(grade), bank: new Float32Array(z.length), widen: new Float32Array(z.length), side: new Float32Array(z.length) },
});

describe('the curve of a part', () => {
    it('leaves a straight level part alone', () => {
        expect(curveOf(part([0, 0, 500, 0], [0, 0], [10, 10], [0, 0]), 0, ROAD_SAG, 8, () => 0.2).xy).toEqual([0, 0, 500, 0]);
    });

    it('follows a quarter circle and the grades over a hill, and ends on the vertices', () => {
        // A radius of 500 tile units from east to south, in world y down.
        const { xy, z } = curveOf(part([0, 0, 500, 500], [0, Math.PI / 2], [100, 100], [0.1, -0.1]), 0, ROAD_SAG, 8, () => 0.2);
        const n = z.length;
        expect(n).toBeGreaterThan(5);
        expect([xy[2 * n - 2], xy[2 * n - 1], z[n - 1]]).toEqual([500, 500, 100]);
        for (let v = 1; v < n - 1; v++) expect(Math.abs(Math.hypot(xy[2 * v], xy[2 * v + 1] - 500) - 500)).toBeLessThan(40);
        expect(Math.max(...Array.from({ length: n }, (_, v) => z[v]))).toBeGreaterThan(100 + 0.1 * 0.2 * 707 / 4 * 0.9);
    });

    it('cuts a bend finer for a wider road, so that a chord of its edge stays within the sag', () => {
        const bend = part([0, 0, 500, 500], [0, Math.PI / 2], [100, 100], [0, 0]), width = 6;
        const { xy, anchors } = curveOf(bend, width, ROAD_SAG, 8, () => 0.2);
        expect(xy.length).toBeGreaterThan(curveOf(bend, 0, ROAD_SAG, 8, () => 0.2).xy.length);
        expect(anchors.filter(Boolean).length).toBe(2);
        const heading = (v: number) => Math.atan2(xy[2 * v + 3] - xy[2 * v + 1], xy[2 * v + 2] - xy[2 * v]);
        for (let v = 1; 2 * v + 3 < xy.length; v++) {
            const turn = heading(v) - heading(v - 1), chord = Math.hypot(xy[2 * v + 2] - xy[2 * v], xy[2 * v + 3] - xy[2 * v + 1]) * 0.2;
            expect((chord + width * turn) * turn / 8).toBeLessThan(0.07);
        }
    });

    it('holds a chord within the sag on spans that bend at one end, in an S and through most of a turn', () => {
        const metres = 0.2, c = 800, span = (a: number, b: number) => part([0, 0, c, 0], [a, b], [100, 140], [0.05 + a / 5, 0.05 + b / 5]);
        // The curve of the spec, sampled finely. The part runs along x, so y is what a chord of the plan misses.
        const exact = (u: number, a: number, b: number) => {
            const h10 = u ** 3 - 2 * u ** 2 + u, h11 = u ** 3 - u ** 2, h00 = 2 * u ** 3 - 3 * u ** 2 + 1, h01 = 3 * u ** 2 - 2 * u ** 3;
            return { x: h01 * c + (h10 * Math.cos(a) + h11 * Math.cos(b)) * c, y: (h10 * Math.sin(a) + h11 * Math.sin(b)) * c, z: h00 * 100 + h01 * 140 + (h10 * (0.05 + a / 5) + h11 * (0.05 + b / 5)) * c * metres };
        };
        for (const [a, b] of [[0.5, 0], [0, -0.5], [0.3, 0.3], [-0.3, -0.3], [0.4, -0.4], [1.4, -1.4], [1.2, 0], [0, -1.4], [1, 1], [-1.4, 0.7], [1.5, -1.5]]) {
            const { xy, z } = curveOf(span(a, b), 0, ROAD_SAG, 8, () => metres);
            let plan = 0, height = 0;
            for (let k = 0; k <= 800; k++) {
                const p = exact(k / 800, a, b);
                let nearest = Infinity, level = 0;
                for (let v = 0; v + 1 < z.length; v++) {
                    const dx = xy[2 * v + 2] - xy[2 * v], dy = xy[2 * v + 3] - xy[2 * v + 1], t = Math.max(0, Math.min(1, ((p.x - xy[2 * v]) * dx + (p.y - xy[2 * v + 1]) * dy) / (dx * dx + dy * dy)));
                    const d = Math.hypot(xy[2 * v] + t * dx - p.x, xy[2 * v + 1] + t * dy - p.y);
                    if (d < nearest) [nearest, level] = [d, z[v] + t * (z[v + 1] - z[v])];
                }
                plan = Math.max(plan, nearest * metres);
                height = Math.max(height, Math.abs(level - p.z));
            }
            // Half a world unit of rounding beside the sag.
            expect(plan, `plan ${a}, ${b}`).toBeLessThan(1.25 * ROAD_SAG.plan + 0.5 / 8 * metres);
            expect(height, `height ${a}, ${b}`).toBeLessThan(1.25 * ROAD_SAG.height);
        }
    });

    it('carries the plan along the curve: its tangent and grade, and a bank and a widening that run from vertex to vertex', () => {
        const bend = part([0, 0, 500, 500], [0, Math.PI / 2], [100, 100], [0.1, -0.1]);
        bend.plan!.bank.set([0, 0.06]);
        bend.plan!.widen.set([1, 0]);
        const { xy, z, plan } = curveOf(bend, 3, ROAD_SAG, 8, () => 0.2), n = z.length;
        expect([plan.tangent.length, plan.grade.length, plan.bank.length, plan.widen.length]).toEqual([n, n, n, n]);
        for (let v = 1; v + 1 < n; v++) {
            const dx = xy[2 * v + 2] - xy[2 * v - 2], dy = xy[2 * v + 3] - xy[2 * v - 1];
            expect(Math.abs(Math.atan2(dy, dx) - plan.tangent[v])).toBeLessThan(0.02);
            expect(plan.grade[v]).toBeCloseTo((z[v + 1] - z[v - 1]) / (Math.hypot(dx, dy) * 0.2), 2);
            expect(plan.bank[v] / 0.06 + plan.widen[v]).toBeCloseTo(1, 5);
        }
    });

    it('keeps the height of a vertex beside one with no height', () => {
        const { z } = curveOf(part([0, 0, 500, 0, 1000, 0, 1500, 0], [0, 0, 0, 0], [NaN, 100, 100, NaN], [0, 0, 0, 0]), 0, ROAD_SAG, 8, () => 0.2);
        expect(z).toEqual([NaN, 100, 100, NaN]);
    });

    it('cuts a segment alike in each tile that holds it, whichever vertex comes first', () => {
        // A curved segment, alone and after a segment far to the north: the scale grows to the north.
        const metres = (y: number) => 0.2 * (1 + y / 200000);
        const bend = (ax: number, ay: number) => [ax, ay, ax + 900, ay + 300];
        const alone = curveOf(part(bend(0, 50000), [0.1, 0.5], [100, 110], [0.05, -0.02]), 0, ROAD_SAG, 8, metres);
        const after = curveOf(part([0, 0, ...bend(0, 50000)], [0, 0.1, 0.5], [90, 100, 110], [0, 0.05, -0.02]), 0, ROAD_SAG, 8, metres);
        expect(after.z.slice(-alone.z.length)).toEqual(alone.z);
        expect(after.xy.slice(-alone.xy.length)).toEqual(alone.xy);
    });
});
