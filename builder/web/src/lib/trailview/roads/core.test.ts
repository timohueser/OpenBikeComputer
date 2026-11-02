import { describe, expect, it } from 'vitest';
import { metresPerUnit, PROFILES, ROAD, type Line } from '../terrain/bins';
import { LEFT } from '../terrain/town';
import { coreEdge, inCore } from './core';
import { stationsOf } from './stations';

describe('the core of a way', () => {
    // A residential road 100 m east along world row 2^29, with a sidewalk on its left (north).
    const metres = metresPerUnit(2 ** 29), zero = [0, 0];
    const line: Line = {
        kind: ROAD, xy: [2 ** 28, 2 ** 29, 2 ** 28 + Math.round(100 / metres), 2 ** 29], z: [1000, 1000], h: 3, code: 2, ford: false, profile: Object.keys(PROFILES).indexOf('residential'),
        anchors: [true, true], plan: { tangent: zero, grade: zero, bank: zero, widen: zero }, sidewalks: [LEFT],
    };
    const stations = stationsOf(line, () => line.h), s = stations[Math.floor(stations.length / 2)];
    const at = (across: number, ahead = 0) => [s.x + s.ax * across - s.ay * ahead, s.y + s.ay * across + s.ax * ahead] as const;

    it('reaches out to the edge of each side at the nearest station, past the sidewalk on the left', () => {
        const [left, right] = [coreEdge(line, s, 1), coreEdge(line, s, -1)];
        expect(left).toBeGreaterThan(right);
        expect(right).toBeGreaterThanOrEqual(line.h);
        for (const [across, inside] of [[left - 0.05, true], [left + 0.05, false], [-right + 0.05, true], [-right - 0.05, false]] as const) {
            expect(inCore(line, stations, ...at(across)), `${across} m across`).toBe(inside);
        }
    });

    it('ends at its first and last station', () => {
        const [first, last] = [stations[0], stations[stations.length - 1]];
        expect(inCore(line, stations, first.x - first.ay * -0.5, first.y + first.ax * -0.5)).toBe(false);
        expect(inCore(line, stations, last.x - last.ay * 0.5, last.y + last.ax * 0.5)).toBe(false);
        expect(inCore(line, stations, last.x - last.ay * -0.5, last.y + last.ax * -0.5)).toBe(true);
    });
});
