import { describe, expect, it } from 'vitest';
import { polyline } from './curve';
import type { FeaturePart } from './features';

const part = (xy: number[], tangent: number[], z: number[], grade: number[]): FeaturePart => ({
    outer: false, xy: Float32Array.from(xy), z: Float32Array.from(z),
    plan: { along: 0, backward: false, tangent: Float32Array.from(tangent), grade: Float32Array.from(grade), bank: new Float32Array(z.length), widen: new Float32Array(z.length) },
});

describe('the curve of a part', () => {
    it('leaves a straight level part alone, and returns a part with no alignment as it is', () => {
        expect(Array.from(polyline(part([0, 0, 500, 0], [0, 0], [10, 10], [0, 0]), () => 0.2).xy)).toEqual([0, 0, 500, 0]);
        const plain: FeaturePart = { outer: false, xy: Int32Array.from([0, 0, 5, 5]), z: null, plan: null };
        expect(polyline(plain, () => 0.2)).toEqual({ xy: plain.xy, z: null });
    });

    it('follows a quarter circle and the grades over a hill, and ends on the vertices', () => {
        // A radius of 500 tile units from east to south, in world y down.
        const { xy, z } = polyline(part([0, 0, 500, 500], [0, Math.PI / 2], [100, 100], [0.1, -0.1]), () => 0.2);
        const n = z!.length;
        expect(n).toBeGreaterThan(5);
        expect([xy[2 * n - 2], xy[2 * n - 1], z![n - 1]]).toEqual([500, 500, 100]);
        for (let v = 1; v < n - 1; v++) expect(Math.abs(Math.hypot(xy[2 * v], xy[2 * v + 1] - 500) - 500)).toBeLessThan(40);
        expect(Math.max(...Array.from({ length: n }, (_, v) => z![v]))).toBeGreaterThan(100 + 0.1 * 0.2 * 707 / 4 * 0.9);
    });

    it('keeps the height of a vertex beside one with no height', () => {
        const { z } = polyline(part([0, 0, 500, 0, 1000, 0, 1500, 0], [0, 0, 0, 0], [NaN, 100, 100, NaN], [0, 0, 0, 0]), () => 0.2);
        expect(Array.from(z!)).toEqual([NaN, 100, 100, NaN]);
    });

    it('cuts a segment alike in each tile that holds it, whichever vertex comes first', () => {
        // A curved segment, alone and after a segment far to the north: the scale grows to the north.
        const metres = (y: number) => 0.2 * (1 + y / 200000);
        const bend = (ax: number, ay: number) => [ax, ay, ax + 900, ay + 300];
        const alone = polyline(part(bend(0, 50000), [0.1, 0.5], [100, 110], [0.05, -0.02]), metres);
        const after = polyline(part([0, 0, ...bend(0, 50000)], [0, 0.1, 0.5], [90, 100, 110], [0, 0.05, -0.02]), metres);
        expect(Array.from(after.z!).slice(-alone.z!.length)).toEqual(Array.from(alone.z!));
        expect(Array.from(after.xy).slice(-alone.xy.length)).toEqual(Array.from(alone.xy));
    });
});
