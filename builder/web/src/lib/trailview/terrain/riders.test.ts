import { describe, expect, it } from 'vitest';
import { KERB_RISE_M, metresPerUnit, PATH, PROFILES, ROAD, type Line } from './bins';
import { alongOf } from './junction-plan';
import { riders } from './riders';
import { LEFT, RIGHT } from './town';

const X = 2 ** 28, Y = 2 ** 29, unit = metresPerUnit(Y), line = (kind: number, name: string, code: number, h: number, points: [number, number][], z: number, extra: Partial<Line> = {}): Line => ({
    kind, xy: points.flatMap(([e, n]) => [X + Math.round(e / unit), Y - Math.round(n / unit)]), z: points.map(() => z), h, code, ford: false, profile: Object.keys(PROFILES).indexOf(name), ...extra,
});
const road = () => line(ROAD, 'residential', 2, 3, [[-100, 0], [100, 0]], 1000, { sidewalks: [LEFT | RIGHT] });
const beside = (z: number, north = 3.5) => line(PATH, 'footway', 6, 0.6, [[-60, 12], [-40, north], [40, north], [60, 12]], z);

describe('a path that OSM maps inside a road\'s core', () => {
    it('lies on the sidewalk, is inside where its whole core is, and returns to its own heights at a walkable grade', () => {
        const path = beside(998.5);
        riders([road(), path]);
        const metres = alongOf(path), on = metres.map((_, v) => v).filter(v => Math.abs((path.xy[2 * v + 1] - Y) * unit + 3.5) < 0.05);
        for (const v of on) expect(path.z[v]).toBeCloseTo(1000 + KERB_RISE_M, 2);
        expect(path.inside!.filter(Boolean).length).toBeGreaterThan(0);
        for (let v = 0; v + 1 < metres.length; v++) if (path.inside![v]) expect([path.z[v], path.z[v + 1]]).toEqual([expect.closeTo(1000 + KERB_RISE_M, 2), expect.closeTo(1000 + KERB_RISE_M, 2)]);
        expect(path.z[0]).toBe(998.5);
        expect(path.z[path.z.length - 1]).toBe(998.5);
        for (let v = 0; v + 1 < path.z.length; v++) expect(Math.abs(path.z[v + 1] - path.z[v]) / (metres[v + 1] - metres[v])).toBeLessThan(0.25);
    });

    it('leaves a path alone that lies off the road, or far under its surface', () => {
        for (const path of [beside(998.5, 6), beside(990)]) {
            const z = path.z.slice();
            riders([road(), path]);
            expect(path.inside).toBeUndefined();
            expect(path.z).toEqual(z);
        }
    });

    it('keeps the height of a vertex that it shares with another way past the road', () => {
        const path = beside(998.5), other = line(PATH, 'footway', 6, 0.6, [[60, 12], [80, 20]], 998.5);
        path.xy.splice(6, 2, ...other.xy.slice(0, 2));
        riders([road(), path, other]);
        expect(path.z[path.z.length - 1]).toBe(998.5);
    });

    it('agrees in two tiles that clip the line elsewhere', () => {
        const whole = beside(998.5), clipped = line(PATH, 'footway', 6, 0.6, [[-40, 3.5], [40, 3.5], [60, 12]], 998.5);
        riders([road(), whole]);
        riders([road(), clipped]);
        const heightAt = (l: Line, x: number) => l.z[l.xy.findIndex((value, i) => i % 2 === 0 && Math.abs(value - x) < 1) / 2];
        expect(heightAt(clipped, whole.xy[whole.xy.length - 2])).toBe(heightAt(whole, whole.xy[whole.xy.length - 2]));
        expect(heightAt(clipped, X + Math.round(40 / unit))).toBeCloseTo(heightAt(whole, X + Math.round(40 / unit)), 6);
    });

    it('leaves a path alone that runs on from a road\'s end, past a short last segment', () => {
        // The fixture's road at 46.8238, 8.4052 ends in a segment of 1.7 m; a disc about its last but one vertex would reach 2.3 m past its end.
        const end = line(ROAD, 'residential', 2, 2.5, [[-100, 0], [-1.7, 0], [0, 0]], 1000, { sidewalks: [LEFT | RIGHT, LEFT | RIGHT] });
        const path = line(PATH, 'path', 4, 1, [[0, 0], [60, 0]], 1000);
        riders([end, path]);
        expect(path.inside).toBeUndefined();
        expect(path.z).toEqual([1000, 1000]);
    });

    it('takes a road with no sidewalk on that side up to its shoulder only', () => {
        const path = beside(998.5);
        riders([{ ...road(), sidewalks: [RIGHT] }, path]);
        expect(path.inside).toBeUndefined();
    });
});
