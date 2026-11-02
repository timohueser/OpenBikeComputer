import { describe, expect, it } from 'vitest';
import { metresPerUnit, type Line } from '../terrain/bins';
import { curveOf, pointOn, ROAD_SAG, spanOf } from '../stream/curve';
import type { FeaturePart } from '../stream/features';
import { farStations, reachOf, STEP_M, stationsOf } from './stations';

/** A line along a circle of `radius` metres from east to the south (world y runs south), `turn` radians long, level, through vertices `chord` metres apart, with its alignment. */
function bend(radius: number, turn: number, chord: number, y = 2 ** 29): Line {
    const unit = metresPerUnit(y), count = Math.max(2, Math.ceil(radius * turn / chord)), xy: number[] = [], tangent: number[] = [];
    for (let v = 0; v <= count; v++) {
        const a = turn * v / count;
        xy.push(Math.round(2 ** 28 + radius * Math.sin(a) / unit), Math.round(y + radius * (1 - Math.cos(a)) / unit));
        tangent.push(a);
    }
    const zero = tangent.map(() => 0);
    return { kind: 1, xy, z: zero.map(() => 1000), h: 3, code: 2, ford: false, profile: 0, anchors: tangent.map(() => true), plan: { tangent, grade: zero, bank: zero, widen: zero } };
}
const half = () => 3, side = 2 ** 14;

describe('the stations of a line', () => {
    it('keep to 2 m and 2° and hold 1 m apart inside a line, on the curve that the stamp cuts into chords', () => {
        const line = bend(30, Math.PI / 2, 12), stations = stationsOf(line, half), unit = metresPerUnit(2 ** 29);
        expect(stations[0]).toMatchObject({ x: line.xy[0], y: line.xy[1] });
        expect(stations.at(-1)).toMatchObject({ x: line.xy.at(-2), y: line.xy.at(-1) });
        for (let j = 0; j + 1 < stations.length; j++) {
            const s = stations[j], t = stations[j + 1], step = Math.hypot(t.x - s.x, t.y - s.y) * unit, turn = Math.acos(Math.min(1, (s.ax * t.ax + s.ay * t.ay) / Math.hypot(s.ax, s.ay) / Math.hypot(t.ax, t.ay)));
            expect(step).toBeLessThanOrEqual(STEP_M + 1e-3);
            // A step short of a vertex of the tile may be shorter.
            if (!line.xy.some((_, i) => i % 2 === 0 && line.xy[i] === t.x && line.xy[i + 1] === t.y)) expect(step).toBeGreaterThan(0.9);
            expect(turn).toBeLessThan(2.2 * Math.PI / 180);
            expect(t.along - s.along).toBeCloseTo(step, 1);
        }
        for (const s of stations) expect(Math.abs(Math.hypot(s.x - line.xy[0], s.y - line.xy[1] - 30 / unit) * unit - 30)).toBeLessThan(0.03);
    });

    it('lie on the curve of the alignment, between the points of the chords that the stamp holds', () => {
        const part: FeaturePart = { outer: false, xy: Float32Array.from([0, 0, 500, 500]), z: Float32Array.from([100, 100]), plan: { along: 0, backward: false, tangent: Float32Array.from([0, Math.PI / 2]), grade: new Float32Array(2), bank: new Float32Array(2), widen: new Float32Array(2), side: new Float32Array(2) } };
        const scale = 8, unit = metresPerUnit(2 ** 29), { xy, z, anchors, plan } = curveOf(part, 0, ROAD_SAG, scale, () => scale * unit);
        const line: Line = { kind: 1, xy: xy.map((value, i) => value * scale + (i % 2) * 2 ** 29), z, h: 3, code: 2, ford: false, profile: 0, anchors, plan };
        const stations = stationsOf(line, half);
        expect(stations.length).toBeGreaterThan(z.length);
        const span = spanOf(4000, 4000, 0, Math.PI / 2, 100, 100, 0, 0, 4000 * Math.SQRT2 * unit), curve = Array.from({ length: 20001 }, (_, k) => pointOn(span, k / 20000));
        for (const s of stations) expect(Math.min(...curve.map(({ x, y }) => Math.hypot(x - s.x, y + 2 ** 29 - s.y))) * unit).toBeLessThan(0.03);
    });

    it('take the bank of the stamp, whose tilts rise to the right', () => {
        const line = bend(1000, 0.01, 20), rising = { ...line, plan: { ...line.plan!, bank: line.plan!.bank.map(() => 0.05) } };
        expect(stationsOf(rising, half)[1].bank).toBeCloseTo(0.05);
    });

    it('start again on each vertex of the tile, so a line cut short at its start keeps the stations of the rest', () => {
        const line = bend(40, Math.PI / 2, 9), from = 3, rest: Line = {
            ...line, xy: line.xy.slice(2 * from), z: line.z.slice(from), anchors: line.anchors!.slice(from),
            plan: { tangent: line.plan!.tangent.slice(from), grade: line.plan!.grade.slice(from), bank: line.plan!.bank.slice(from), widen: line.plan!.widen.slice(from) },
        };
        const whole = stationsOf(line, half), part = stationsOf(rest, half), at = whole.findIndex(({ x, y }) => x === rest.xy[0] && y === rest.xy[1]);
        expect(at).toBeGreaterThan(0);
        expect(part.map(({ x, y }) => [x, y])).toEqual(whole.slice(at).map(({ x, y }) => [x, y]));
    });

    it('stand at each change of the sidewalks, so a side ends on a station', () => {
        const line = { ...bend(10000, 0.01, 20), sidewalks: [0, 3, 3, 0, 0] };
        const stations = stationsOf(line, half), change = stations.findIndex((s, j) => j > 0 && line.sidewalks[s.v] !== line.sidewalks[stations[j - 1].v]);
        const at = stations[change - 1];
        expect([at.x, at.y]).toEqual(line.xy.slice(2 * (at.v + 1), 2 * (at.v + 1) + 2));
    });
});

describe('the reach of a station', () => {
    it('stops a side short of a tight bend\'s middle on its inside, and leaves its outside free', () => {
        const stations = stationsOf(bend(4, Math.PI, 2), () => 1.5), reach = reachOf(stations), middle = 2 * (stations.length >> 1);
        // The bend turns right: its inside is on the right.
        expect(reach[middle + 1]).toBeGreaterThan(3);
        expect(reach[middle + 1]).toBeLessThan(4);
        expect(reach[middle]).toBe(Infinity);
    });
});

describe('the far range', () => {
    it('keeps the first station in each square, where a near range ends', () => {
        const stations = stationsOf(bend(1e6, 1e-2, 40), half), far = new Set(farStations({ z: [] } as unknown as Line, stations, side));
        const entries = stations.flatMap(({ x }, j) => (j > 0 && Math.floor(x / side) !== Math.floor(stations[j - 1].x / side) ? [j] : []));
        expect(entries.length).toBeGreaterThan(0);
        for (const j of entries) expect(far.has(j)).toBe(true);
    });

    it('keeps the ends, skips most of a straight, and keeps a bend within its sag', () => {
        const straight = stationsOf(bend(1e6, 1e-4, 40, 2 ** 29 + 1000), half), bent = stationsOf(bend(60, Math.PI / 2, 10), half);
        const far = farStations({ z: [] } as unknown as Line, straight, 2 ** 20);
        expect(far[0]).toBe(0);
        expect(far.at(-1)).toBe(straight.length - 1);
        expect(far.length).toBeLessThan(straight.length / 4);
        const curved = farStations({ z: [] } as unknown as Line, bent, 2 ** 20);
        expect(curved.length).toBeLessThan(0.6 * bent.length);
        const unit = metresPerUnit(2 ** 29);
        for (let k = 0; k + 1 < curved.length; k++) {
            const [s, t] = [bent[curved[k]], bent[curved[k + 1]]], chord = Math.hypot(t.x - s.x, t.y - s.y);
            for (let j = curved[k] + 1; j < curved[k + 1]; j++) {
                const p = bent[j], off = Math.abs((t.x - s.x) * (p.y - s.y) - (t.y - s.y) * (p.x - s.x)) / chord * unit;
                expect(off).toBeLessThanOrEqual(0.1);
            }
        }
    });
});
