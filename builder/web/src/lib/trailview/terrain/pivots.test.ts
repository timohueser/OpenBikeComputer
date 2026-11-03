import { describe, expect, it } from 'vitest';
import { metresPerUnit, PROFILES, ROAD, shoulderOf, type Line } from './bins';
import { FAN_A, FAN_B, fanWord, pivotsOf, PIVOT_STEP_M } from './pivots';

const metres = metresPerUnit(2 ** 29);
/** A residential road 3 m wide on each side: 20 m east, an arc of `radius` metres that turns right by 150°, and 20 m on, rising at 0.1. */
const hairpin = (radius: number): Line => {
    const xy: number[] = [], tangent: number[] = [], turn = (5 * Math.PI) / 6, at = (x: number, y: number, heading: number) => {
        xy.push(2 ** 28 + Math.round(x / metres), 2 ** 29 + Math.round(y / metres));
        tangent.push(heading);
    };
    for (let x = -20; x < 0; x += 2) at(x, 0, 0);
    const steps = Math.ceil((turn * radius) / 0.5);
    for (let k = 0; k <= steps; k++) at(radius * Math.sin((turn * k) / steps), radius * (1 - Math.cos((turn * k) / steps)), (turn * k) / steps);
    for (let d = 2; d <= 20; d += 2) at(radius * Math.sin(turn) + d * Math.cos(turn), radius * (1 - Math.cos(turn)) + d * Math.sin(turn), turn);
    const z = tangent.map((_, v) => 1000 + 0.1 * v), zero = tangent.map(() => 0);
    return { kind: ROAD, xy, z, h: 3, code: 2, ford: false, profile: Object.keys(PROFILES).indexOf('residential'), anchors: tangent.map(() => true), plan: { tangent, grade: zero, bank: zero, widen: zero } };
};
/** The line from vertex v on, as a tile that cuts it there holds it. */
const from = (line: Line, v: number): Line => ({ ...line, xy: line.xy.slice(2 * v), z: line.z.slice(v), anchors: line.anchors!.slice(v), plan: { tangent: line.plan!.tangent.slice(v), grade: line.plan!.grade.slice(v), bank: line.plan!.bank.slice(v), widen: line.plan!.widen.slice(v) } });

describe('the pivots of a line', () => {
    it('lie where the bend is tighter than the core and a radius, the core\'s half-width and r or more from every vertex, and nowhere else', () => {
        for (const [radius, count] of [[2, 1], [4, 1], [8, 0]] as const) {
            const line = hairpin(radius), pivots = pivotsOf(line, () => line.h);
            expect(pivots.length, `radius ${radius}`).toBe(count);
            for (const { x, y, r, side } of pivots) {
                expect(side).toBe(-1);
                for (let v = 0; v < line.z.length; v++) expect(Math.hypot(line.xy[2 * v] - x, line.xy[2 * v + 1] - y) * metres).toBeGreaterThan(line.h + shoulderOf(line) + r - 0.06);
            }
        }
    });

    it('are the same in every tile that holds the bend: a line cut 6 m before it finds them at the same point', () => {
        for (const radius of [2, 4]) {
            const line = hairpin(radius), [whole] = pivotsOf(line, () => line.h), at = whole.first - 3, [cut] = pivotsOf(from(line, at), () => line.h);
            expect(at, `radius ${radius}`).toBeGreaterThan(0);
            expect(cut.r).toBe(whole.r);
            expect([cut.first + at, cut.last + at]).toEqual([whole.first, whole.last]);
            expect(Math.hypot(cut.x - whole.x, cut.y - whole.y) * metres).toBeLessThan(1e-6);
        }
    });

    it('pack each segment of a fan for the stamp: the pivot about its start, the radius, and the ends that point at it', () => {
        const line = hairpin(4), pivots = pivotsOf(line, () => line.h), [{ x, y, r, first, last }] = pivots;
        for (let v = 0; v + 1 < line.z.length; v++) {
            const word = fanWord(line, pivots, v);
            expect(word !== 0, `segment ${v}`).toBe(v + 1 >= first && v <= last);
            if (!word) continue;
            const q = [(word << 21) >> 21, (word << 10) >> 21].map(c => (c * PIVOT_STEP_M) / metres);
            expect(Math.hypot(line.xy[2 * v] + q[0] - x, line.xy[2 * v + 1] + q[1] - y) * metres).toBeLessThan(PIVOT_STEP_M);
            expect(((word >>> 22) & 255) / 100).toBe(r);
            expect([(word & FAN_A) !== 0, word >= FAN_B]).toEqual([v >= first, v + 1 <= last]);
        }
    });
});
